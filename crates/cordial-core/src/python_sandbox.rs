//! Experimental WASI Python calculator, not enabled in the desktop app.
//!
//! This runs only the pinned CPython WASI module, not arbitrary wasm. No host
//! directories, environment, inherited descriptors, processes or network are
//! supplied. Code is passed as an argument and is never written as a script.
//! Isolation depends on Wasmtime, the guest, and the operating system; it is
//! not an absolute security guarantee. This prototype needs cross-platform
//! evaluation before it can be enabled in a release.

use sha2::{Digest, Sha256};
use std::path::Path;
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{
    p2::pipe::MemoryOutputPipe,
    preview1::{self, WasiP1Ctx},
    I32Exit, WasiCtxBuilder,
};

const MAX_SCRIPT_BYTES: usize = 8_192;
const MAX_OUTPUT_BYTES: usize = 32_768;
const MAX_MODULE_BYTES: u64 = 27_000_000;
const MODULE_SHA256: &str = "e5dc5a398b07b54ea8fdb503bf68fb583d533f10ec3f930963e02b9505f7a763";
const FUEL: u64 = 1_000_000_000;

struct State {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// A compiled, integrity-checked CPython guest. Its fields are private so
/// callers cannot substitute an untrusted module.
pub struct PythonSandbox {
    engine: Engine,
    module: Module,
}

impl PythonSandbox {
    /// Compiles the pinned guest once for reuse across isolated invocations.
    pub fn load(path: &Path) -> Result<Self, String> {
        let metadata =
            std::fs::metadata(path).map_err(|e| format!("Python runtime unavailable: {e}"))?;
        if !metadata.is_file() || metadata.len() > MAX_MODULE_BYTES {
            return Err("Invalid Python runtime size or type".into());
        }
        use std::io::Read;
        let file =
            std::fs::File::open(path).map_err(|e| format!("Cannot read Python runtime: {e}"))?;
        let mut bytes = Vec::new();
        file.take(MAX_MODULE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read Python runtime: {e}"))?;
        if bytes.len() as u64 > MAX_MODULE_BYTES
            || format!("{:x}", Sha256::digest(&bytes)) != MODULE_SHA256
        {
            return Err("Python runtime integrity check failed".into());
        }
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|e| format!("Cannot initialize sandbox: {e}"))?;
        let module = Module::new(&engine, bytes)
            .map_err(|e| format!("Cannot compile Python runtime: {e}"))?;
        Ok(Self { engine, module })
    }

    /// Execute in a fresh capability-free WASI instance. Call only from a
    /// blocking thread: the synchronous WASI adapter must not run on a Tokio worker.
    pub(crate) fn run(&self, script: &str) -> Result<String, String> {
        let engine = &self.engine;
        let module = &self.module;
        if script.is_empty() || script.len() > MAX_SCRIPT_BYTES || script.contains('\0') {
            return Err("Python script must be 1-8192 bytes without NULs".into());
        }
        let stdout = MemoryOutputPipe::new(MAX_OUTPUT_BYTES);
        let stderr = MemoryOutputPipe::new(MAX_OUTPUT_BYTES);
        let wasi = WasiCtxBuilder::new()
            .args(&["python", "-c", script])
            .stdout(stdout.clone())
            .stderr(stderr.clone())
            .build_p1();
        let limits = StoreLimitsBuilder::new()
            .memory_size(128 * 1024 * 1024)
            .instances(1)
            .tables(4)
            .build();
        let mut store = Store::new(engine, State { wasi, limits });
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(FUEL)
            .map_err(|e| format!("Cannot set execution budget: {e}"))?;
        let mut linker = Linker::new(engine);
        preview1::add_to_linker_sync(&mut linker, |state: &mut State| &mut state.wasi)
            .map_err(|e| format!("Cannot configure sandbox: {e}"))?;
        // Fuel only counts guest instructions; a WASI poll may otherwise sleep
        // inside the host for an arbitrary time without consuming fuel. The
        // calculator has no need for timers or descriptor polling.
        linker.allow_shadowing(true);
        linker
            .func_wrap(
                "wasi_snapshot_preview1",
                "poll_oneoff",
                |_: i32, _: i32, _: i32, _: i32| -> i32 { 58 },
            )
            .map_err(|e| format!("Cannot disable blocking WASI polls: {e}"))?;
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| format!("Cannot start sandbox: {e}"))?;
        let start = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .map_err(|e| format!("Invalid Python runtime: {e}"))?;
        let result = start.call(&mut store, ());
        let output_bytes = stdout.contents();
        let error_bytes = stderr.contents();
        let output = String::from_utf8_lossy(&output_bytes).into_owned();
        let errors = String::from_utf8_lossy(&error_bytes).into_owned();
        if output_bytes.len() >= MAX_OUTPUT_BYTES || error_bytes.len() >= MAX_OUTPUT_BYTES {
            return Err("Python output exceeded the sandbox limit".into());
        }
        match result {
            Ok(()) => Ok(output),
            Err(error)
                if error
                    .downcast_ref::<I32Exit>()
                    .is_some_and(|exit| exit.0 == 0) =>
            {
                Ok(output)
            }
            Err(error) => Err(format!(
                "Python failed or exceeded a sandbox limit: {error}\n{errors}\n{output}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_python_executes_calculations_and_refuses_host_access() {
        let path = std::env::var("CORDIAL_TEST_PYTHON_WASM")
            .expect("sandbox-python tests require CORDIAL_TEST_PYTHON_WASM");
        let sandbox = PythonSandbox::load(Path::new(&path)).expect("load pinned WASI runtime");
        let output = sandbox
            .run("import math\nprint(f'{24*math.pi:.2f}')\nprint(f'{(8/math.sqrt(3))**3:.3f}')")
            .expect("run isolated calculations");
        assert_eq!(output.trim(), "75.40\n98.534");
        let output = sandbox.run("import os\nprint(len(os.environ))\ntry:\n open('/etc/passwd').read()\n print('HOST ACCESS')\nexcept OSError:\n print('DENIED')")
            .expect("run filesystem probe");
        assert_eq!(output.trim(), "0\nDENIED");
        assert!(
            sandbox.run("while True: pass").is_err(),
            "infinite loop escaped fuel limit"
        );
        assert!(
            sandbox.run("print('x' * 1000000)").is_err(),
            "output limit not enforced"
        );
        assert!(
            sandbox.run("import time; time.sleep(60)").is_err(),
            "blocking WASI polling was allowed"
        );
        assert!(
            sandbox.run("import socket; socket.socket()").is_err(),
            "network socket was allowed"
        );
        assert!(
            sandbox.run("x = bytearray(1024 * 1024 * 1024)").is_err(),
            "memory limit was not enforced"
        );
        assert!(
            sandbox.run("import os; os.system('echo unsafe')").is_err(),
            "process creation was allowed"
        );
        sandbox.run("x = 123").expect("first isolated run");
        assert!(
            sandbox.run("print(x)").is_err(),
            "state leaked between instances"
        );
        let sentinel =
            std::env::temp_dir().join(format!("cordial-wasi-sentinel-{}", uuid::Uuid::new_v4()));
        std::fs::write(&sentinel, b"unchanged").unwrap();
        let escaped = sentinel
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        assert!(
            sandbox
                .run(&format!("open('{escaped}', 'w').write('changed')"))
                .is_err(),
            "host write was allowed"
        );
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"unchanged");
        std::fs::remove_file(sentinel).unwrap();
        assert!(sandbox.run(&"a".repeat(MAX_SCRIPT_BYTES + 1)).is_err());
        let bad =
            std::env::temp_dir().join(format!("cordial-wasm-tampered-{}", uuid::Uuid::new_v4()));
        std::fs::write(&bad, b"not the pinned runtime").unwrap();
        assert!(PythonSandbox::load(&bad).is_err());
        std::fs::remove_file(bad).unwrap();

        let root =
            std::env::temp_dir().join(format!("cordial-python-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let runtime = crate::tools::ToolRuntime::new(
            root.clone(),
            root.join("scratch"),
            crate::settings::Effort::Task,
        )
        .unwrap()
        .with_python_sandbox(std::sync::Arc::new(sandbox));
        let call = crate::session::ToolCall {
            id: "test".into(),
            name: "python_calculate".into(),
            arguments: vec![crate::session::ToolArgument {
                name: "code".into(),
                value: "print(6 * 7)".into(),
            }],
        };
        let executor = tokio::runtime::Runtime::new().unwrap();
        let outcome = executor.block_on(runtime.execute(&call));
        assert!(outcome.ok, "{}", outcome.text);
        assert!(outcome.text.contains("42"));
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            0,
            "the tool created a file"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
