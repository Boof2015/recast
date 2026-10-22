use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use wasmtime::{Config, Engine, Instance, Linker, Module, Store};
use wasmtime_wasi::{p1::WasiP1Ctx, FsPerms, WasiCtxBuilder};

pub type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn config() -> Result<Config> {
    let mut config = Config::new();
    // An explicit target disables host CPU feature detection. Packages must
    // also run on machines without the builder's optional CPU instructions.
    config.target(env!("RECAST_TARGET")).map_err(error)?;
    config.epoch_interruption(true);
    config.cranelift_opt_level(wasmtime::OptLevel::SpeedAndSize);
    Ok(config)
}
pub fn compile(source: &Path, destination: &Path) -> Result<()> {
    let engine = Engine::new(&config()?).map_err(error)?;
    let bytes = engine
        .precompile_module(&fs::read(source).map_err(error)?)
        .map_err(|e| format!("{e:#}"))?;
    fs::write(destination, &bytes).map_err(error)?;
    fs::write(
        destination.with_extension("sha256"),
        format!("{:x}", Sha256::digest(&bytes)),
    )
    .map_err(error)
}
pub struct Pandoc {
    engine: Engine,
    store: Store<WasiP1Ctx>,
    instance: Instance,
}
impl Pandoc {
    pub fn new(root: &Path, work: &Path) -> Result<Self> {
        let engine = Engine::new(&config()?).map_err(error)?;
        let bytes = fs::read(root.join("lib/pandoc.cwasm")).map_err(error)?;
        let expected = fs::read_to_string(root.join("lib/pandoc.sha256")).map_err(error)?;
        if format!("{:x}", Sha256::digest(&bytes)) != expected.trim() {
            return Err("The document engine failed its integrity check.".into());
        }
        // SAFETY: only the build-generated artifact shipped in the installed
        // extension is loaded. The host verifies the package inventory before
        // execution; the digest above checks these exact bytes before loading.
        let module = unsafe { Module::deserialize(&engine, &bytes) }
            .map_err(|e| format!("The document engine is incompatible: {e:#}"))?;
        let wasi = WasiCtxBuilder::new()
            .args(&["pandoc.wasm"])
            .preopened_dir(work, "/", FsPerms::ReadWrite)
            .map_err(error)?
            .build_p1();
        let mut store = Store::new(&engine, wasi);
        store.set_epoch_deadline(1);
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |state| state).map_err(error)?;
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| format!("{e:#}"))?;
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
            init.call(&mut store, ()).map_err(error)?;
        }
        instance
            .get_typed_func::<(), ()>(&mut store, "__wasm_call_ctors")
            .map_err(error)?
            .call(&mut store, ())
            .map_err(error)?;
        instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "hs_init_with_rtsopts")
            .map_err(error)?
            .call(&mut store, (0, 0))
            .map_err(error)?;
        Ok(Self {
            engine,
            store,
            instance,
        })
    }
    pub fn engine(&self) -> Engine {
        self.engine.clone()
    }
    pub fn convert(&mut self, options: serde_json::Value, work: &Path) -> Result<()> {
        for name in ["stdout", "stderr", "warnings"] {
            fs::write(work.join(name), b"").map_err(error)?;
        }
        let bytes = serde_json::to_vec(&options).map_err(error)?;
        let pointer = self
            .instance
            .get_typed_func::<i32, i32>(&mut self.store, "malloc")
            .map_err(error)?
            .call(&mut self.store, bytes.len() as i32)
            .map_err(error)?;
        self.instance
            .get_memory(&mut self.store, "memory")
            .ok_or("Missing document engine memory.")?
            .write(&mut self.store, pointer as usize, &bytes)
            .map_err(error)?;
        self.instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, "convert")
            .map_err(error)?
            .call(&mut self.store, (pointer, bytes.len() as i32))
            .map_err(|e| format!("Document conversion stopped: {e:#}"))?;
        let diagnostics = fs::read_to_string(work.join("stderr")).map_err(error)?;
        let warnings = fs::read_to_string(work.join("warnings")).map_err(error)?;
        let entries: Vec<serde_json::Value> = if warnings.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&warnings).map_err(error)?
        };
        let warning = entries
            .iter()
            .find(|entry| entry["verbosity"] != "INFO" && entry["verbosity"] != "DEBUG");
        if !diagnostics.trim().is_empty() || warning.is_some() {
            let message = diagnostics
                .lines()
                .next()
                .filter(|line| !line.is_empty())
                .or_else(|| warning.and_then(|entry| entry["pretty"].as_str()))
                .unwrap_or("The document contains unsupported content.");
            return Err(message.chars().take(600).collect());
        }
        Ok(())
    }
}
