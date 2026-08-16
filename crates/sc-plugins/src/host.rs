//! wasmtime plugin host.
//!
//! Plugins are sandboxed core-WASM modules (`wasm32-unknown-unknown`) using a
//! small stable ABI:
//!
//! Guest exports:
//! - `memory`                                  linear memory
//! - `sc_alloc(len: i32) -> i32`               allocate a guest buffer
//! - `sc_manifest() -> i64`                    packed ptr/len of manifest JSON
//! - `sc_run_command(ptr, len) -> i64`         optional; JSON in (paths), JSON out
//! - `sc_column_value(ptr, len) -> i64`        optional; path in, value string out
//! - `sc_list_archive(ptr, len) -> i64`        optional; path in, JSON entries out
//! - `sc_preview(ptr, len) -> i64`             optional; path in, JSON `{kind,body}` out
//! - `sc_on_event(ptr, len) -> i64`            optional; JSON hook payload in
//!
//! Host imports (module `"sc"`), all capability-checked:
//! - `read_file(ptr, len) -> i64`              read a file the user granted access to
//! - `write_file(path_ptr, path_len, data_ptr, data_len) -> i32`
//! - `log(ptr, len)`
//!
//! Packed i64 return: high 32 bits = ptr, low 32 bits = len; 0 = error/none.
//! Every invocation gets a fresh instance with a fuel budget, so a runaway
//! or crashing plugin cannot hang or corrupt the app.

use crate::manifest::{PluginManifest, PluginRecord, PluginRegistry};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use wasmtime::{Caller, Config, Engine, Extern, Linker, Module, Store, TypedFunc};

const FUEL_BUDGET: u64 = 5_000_000_000;
/// Refuse to hand more than this many bytes into a guest in one read.
const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;
/// Cap plugin file writes so a buggy hook cannot fill the disk.
const MAX_WRITE_BYTES: u64 = 8 * 1024 * 1024;

pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub record: PluginRecord,
    module: Module,
}

impl LoadedPlugin {
    pub fn is_command(&self) -> bool {
        self.manifest.kinds.iter().any(|k| k == "command")
    }
    pub fn is_column(&self) -> bool {
        self.manifest.kinds.iter().any(|k| k == "column")
    }
    pub fn is_preview(&self) -> bool {
        self.manifest.kinds.iter().any(|k| k == "preview")
    }
    pub fn handles_ext(&self, ext: &str) -> bool {
        self.manifest.extensions.is_empty()
            || self.manifest.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
    }
}

struct HostState {
    /// Permissions granted by the user to the running plugin.
    granted_read: bool,
    granted_write: bool,
}

pub struct PluginHost {
    engine: Engine,
    pub plugins: Vec<LoadedPlugin>,
    registry_path: PathBuf,
    pub registry: PluginRegistry,
}

impl PluginHost {
    pub fn new(registry_path: PathBuf) -> Result<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)?;
        let registry = PluginRegistry::load(&registry_path);
        let mut host = Self { engine, plugins: Vec::new(), registry_path, registry };
        let records = host.registry.plugins.clone();
        for rec in records {
            if rec.enabled {
                let _ = host.load_plugin(rec);
            }
        }
        Ok(host)
    }

    /// Install a plugin from a .wasm file: reads its manifest and registers it
    /// (permissions start ungranted unless `grant_all`).
    pub fn install(&mut self, wasm_path: &Path, grant_all: bool) -> Result<PluginManifest> {
        let module = Module::from_file(&self.engine, wasm_path)
            .map_err(|e| anyhow!("compile {}: {e}", wasm_path.display()))?;
        let manifest = self.read_manifest(&module)?;
        let record = PluginRecord {
            path: wasm_path.to_path_buf(),
            enabled: true,
            granted: if grant_all { manifest.permissions.clone() } else { Vec::new() },
        };
        // Replace any prior record for the same path.
        self.registry.plugins.retain(|r| r.path != record.path);
        self.registry.plugins.push(record.clone());
        self.registry.save(&self.registry_path);
        self.plugins.retain(|p| p.record.path != record.path);
        self.plugins.push(LoadedPlugin { manifest: manifest.clone(), record, module });
        Ok(manifest)
    }

    pub fn set_enabled(&mut self, path: &Path, enabled: bool) {
        for r in &mut self.registry.plugins {
            if r.path == path {
                r.enabled = enabled;
            }
        }
        self.registry.save(&self.registry_path);
        if !enabled {
            self.plugins.retain(|p| p.record.path != path);
        } else if let Some(rec) =
            self.registry.plugins.iter().find(|r| r.path == path).cloned()
        {
            let _ = self.load_plugin(rec);
        }
    }

    pub fn grant(&mut self, path: &Path, permission: &str) {
        for r in &mut self.registry.plugins {
            if r.path == path && !r.granted.iter().any(|g| g == permission) {
                r.granted.push(permission.to_string());
            }
        }
        self.registry.save(&self.registry_path);
        for p in &mut self.plugins {
            if p.record.path == path && !p.record.granted.iter().any(|g| g == permission) {
                p.record.granted.push(permission.to_string());
            }
        }
    }

    fn load_plugin(&mut self, record: PluginRecord) -> Result<()> {
        let module = Module::from_file(&self.engine, &record.path)?;
        let manifest = self.read_manifest(&module)?;
        self.plugins.push(LoadedPlugin { manifest, record, module });
        Ok(())
    }

    fn read_manifest(&self, module: &Module) -> Result<PluginManifest> {
        let (mut store, instance) = self.instantiate(module, false, false)?;
        let f: TypedFunc<(), i64> = instance.get_typed_func(&mut store, "sc_manifest")?;
        let packed = f.call(&mut store, ())?;
        let bytes = read_packed(&mut store, &instance, packed)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Run a command plugin against a set of selected paths. Returns the
    /// plugin's output text (shown to the user).
    pub fn run_command(&self, index: usize, paths: &[String]) -> Result<String> {
        let plugin = self.plugins.get(index).ok_or_else(|| anyhow!("no such plugin"))?;
        let granted_read = plugin.record.granted.iter().any(|g| g == "read-files");
        let granted_write = plugin.record.granted.iter().any(|g| g == "write-files");
        let (mut store, instance) = self.instantiate(&plugin.module, granted_read, granted_write)?;
        let input = serde_json::to_vec(paths)?;
        let in_ptr = write_guest(&mut store, &instance, &input)?;
        let f: TypedFunc<(i32, i32), i64> =
            instance.get_typed_func(&mut store, "sc_run_command")?;
        let packed = f.call(&mut store, (in_ptr, input.len() as i32))?;
        let bytes = read_packed(&mut store, &instance, packed)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Compute a custom column value for a file. Returns None on any failure
    /// (a plugin bug must never break the file list).
    pub fn column_value(&self, index: usize, path: &str) -> Option<String> {
        let plugin = self.plugins.get(index)?;
        let granted_read = plugin.record.granted.iter().any(|g| g == "read-files");
        let granted_write = plugin.record.granted.iter().any(|g| g == "write-files");
        let (mut store, instance) = self.instantiate(&plugin.module, granted_read, granted_write).ok()?;
        let input = path.as_bytes();
        let in_ptr = write_guest(&mut store, &instance, input).ok()?;
        let f: TypedFunc<(i32, i32), i64> =
            instance.get_typed_func(&mut store, "sc_column_value").ok()?;
        let packed = f.call(&mut store, (in_ptr, input.len() as i32)).ok()?;
        let bytes = read_packed(&mut store, &instance, packed).ok()?;
        let s = String::from_utf8_lossy(&bytes).into_owned();
        if s.is_empty() { None } else { Some(s) }
    }

    /// Plugin preview for a file. Returns `(kind, body)` where kind is
    /// `text` / `info` / `hex`. Failures are ignored.
    pub fn preview(&self, path: &Path) -> Option<(String, String)> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let path_s = path.to_string_lossy().into_owned();
        for (i, p) in self.plugins.iter().enumerate() {
            if !p.is_preview() || !p.handles_ext(&ext) {
                continue;
            }
            if let Ok(v) = self.run_preview(i, &path_s) {
                return Some(v);
            }
        }
        None
    }

    fn run_preview(&self, index: usize, path: &str) -> Result<(String, String)> {
        let plugin = self.plugins.get(index).ok_or_else(|| anyhow!("no such plugin"))?;
        let granted_read = plugin.record.granted.iter().any(|g| g == "read-files");
        let granted_write = plugin.record.granted.iter().any(|g| g == "write-files");
        let (mut store, instance) = self.instantiate(&plugin.module, granted_read, granted_write)?;
        let input = path.as_bytes();
        let in_ptr = write_guest(&mut store, &instance, input)?;
        let f: TypedFunc<(i32, i32), i64> =
            instance.get_typed_func(&mut store, "sc_preview")?;
        let packed = f.call(&mut store, (in_ptr, input.len() as i32))?;
        let bytes = read_packed(&mut store, &instance, packed)?;
        let v: serde_json::Value = serde_json::from_slice(&bytes)?;
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("info")
            .to_string();
        let body = v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .to_string();
        Ok((kind, body))
    }

    /// Fire `sc_on_event` on every plugin that exports it. Errors are logged.
    pub fn run_hooks(&self, event: &str, sources: &[PathBuf], dests: &[PathBuf]) {
        let payload = serde_json::json!({
            "event": event,
            "sources": sources.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "dests": dests.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        });
        let bytes = match serde_json::to_vec(&payload) {
            Ok(b) => b,
            Err(_) => return,
        };
        for plugin in &self.plugins {
            if let Err(e) = self.run_hook(plugin, &bytes) {
                eprintln!("[plugin] hook {} on {}: {e}", event, plugin.manifest.name);
            }
        }
    }

    fn run_hook(&self, plugin: &LoadedPlugin, input: &[u8]) -> Result<()> {
        let granted_read = plugin.record.granted.iter().any(|g| g == "read-files");
        let granted_write = plugin.record.granted.iter().any(|g| g == "write-files");
        let (mut store, instance) = self.instantiate(&plugin.module, granted_read, granted_write)?;
        let f: TypedFunc<(i32, i32), i64> =
            match instance.get_typed_func(&mut store, "sc_on_event") {
                Ok(f) => f,
                Err(_) => return Ok(()),
            };
        let in_ptr = write_guest(&mut store, &instance, input)?;
        let _ = f.call(&mut store, (in_ptr, input.len() as i32))?;
        Ok(())
    }

    fn instantiate(
        &self,
        module: &Module,
        granted_read: bool,
        granted_write: bool,
    ) -> Result<(Store<HostState>, wasmtime::Instance)> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                granted_read,
                granted_write,
            },
        );
        store.set_fuel(FUEL_BUDGET)?;
        let mut linker: Linker<HostState> = Linker::new(&self.engine);

        linker.func_wrap(
            "sc",
            "log",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                if let Ok(msg) = read_guest_str(&mut caller, ptr, len) {
                    eprintln!("[plugin] {msg}");
                }
            },
        )?;

        linker.func_wrap(
            "sc",
            "read_file",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i64 {
                if !caller.data().granted_read {
                    return 0;
                }
                let Ok(path) = read_guest_str(&mut caller, ptr, len) else {
                    return 0;
                };
                let Ok(meta) = std::fs::metadata(&path) else { return 0 };
                if meta.len() > MAX_READ_BYTES {
                    return 0;
                }
                let Ok(data) = std::fs::read(&path) else { return 0 };
                match write_guest_from_caller(&mut caller, &data) {
                    Ok(p) => pack(p, data.len() as i32),
                    Err(_) => 0,
                }
            },
        )?;

        linker.func_wrap(
            "sc",
            "write_file",
            |mut caller: Caller<'_, HostState>,
             path_ptr: i32,
             path_len: i32,
             data_ptr: i32,
             data_len: i32|
             -> i32 {
                if !caller.data().granted_write {
                    return 0;
                }
                if data_len < 0 || data_len as u64 > MAX_WRITE_BYTES {
                    return 0;
                }
                let Ok(path) = read_guest_str(&mut caller, path_ptr, path_len) else {
                    return 0;
                };
                let Ok(data) = read_guest_bytes(&mut caller, data_ptr, data_len) else {
                    return 0;
                };
                if std::fs::write(&path, &data).is_ok() {
                    1
                } else {
                    0
                }
            },
        )?;

        let instance = linker.instantiate(&mut store, module)?;
        Ok((store, instance))
    }
}

fn pack(ptr: i32, len: i32) -> i64 {
    ((ptr as i64) << 32) | (len as i64 & 0xFFFF_FFFF)
}

fn unpack(packed: i64) -> (i32, i32) {
    (((packed >> 32) & 0xFFFF_FFFF) as i32, (packed & 0xFFFF_FFFF) as i32)
}

fn read_packed(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    packed: i64,
) -> Result<Vec<u8>> {
    if packed == 0 {
        return Err(anyhow!("plugin returned null"));
    }
    let (ptr, len) = unpack(packed);
    let memory = instance
        .get_memory(&mut *store, "memory")
        .ok_or_else(|| anyhow!("plugin has no memory export"))?;
    let mut buf = vec![0u8; len as usize];
    memory.read(&*store, ptr as usize, &mut buf)?;
    Ok(buf)
}

/// Copy bytes into the guest by calling its `sc_alloc`.
fn write_guest(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    data: &[u8],
) -> Result<i32> {
    let alloc: TypedFunc<i32, i32> = instance.get_typed_func(&mut *store, "sc_alloc")?;
    let ptr = alloc.call(&mut *store, data.len() as i32)?;
    let memory = instance
        .get_memory(&mut *store, "memory")
        .ok_or_else(|| anyhow!("plugin has no memory export"))?;
    memory.write(&mut *store, ptr as usize, data)?;
    Ok(ptr)
}

/// Same as `write_guest` but from within a host import (re-entrant call).
fn write_guest_from_caller(caller: &mut Caller<'_, HostState>, data: &[u8]) -> Result<i32> {
    let alloc = caller
        .get_export("sc_alloc")
        .and_then(Extern::into_func)
        .ok_or_else(|| anyhow!("no sc_alloc"))?
        .typed::<i32, i32>(&mut *caller)?;
    let ptr = alloc.call(&mut *caller, data.len() as i32)?;
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| anyhow!("no memory"))?;
    memory.write(&mut *caller, ptr as usize, data)?;
    Ok(ptr)
}

fn read_guest_str(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<String> {
    Ok(String::from_utf8_lossy(&read_guest_bytes(caller, ptr, len)?).into_owned())
}

fn read_guest_bytes(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<Vec<u8>> {
    if len < 0 {
        return Err(anyhow!("negative length"));
    }
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| anyhow!("no memory"))?;
    let mut buf = vec![0u8; len as usize];
    memory.read(&*caller, ptr as usize, &mut buf)?;
    Ok(buf)
}
