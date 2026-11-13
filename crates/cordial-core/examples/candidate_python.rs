//! Evaluation-only bridge to the experimental capability-free Python tool.
//! Model-generated code is never run by the host shell or written to a file.
use cordial_core::session::{ToolArgument, ToolCall};
use cordial_core::settings::Effort;
use cordial_core::tools::ToolRuntime;
use std::io::Read;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut code = String::new();
    std::io::stdin().take(8193).read_to_string(&mut code)?;
    let temp = PathBuf::from(std::env::var("RUNNER_TEMP")?);
    let wasm = PathBuf::from(std::env::var("CORDIAL_TEST_PYTHON_WASM")?);
    let runtime = ToolRuntime::new(temp.clone(), temp, Effort::Task)?.with_python_path(wasm);
    let call = ToolCall {
        id: "evaluation".into(),
        name: "python_calculate".into(),
        arguments: vec![ToolArgument {
            name: "code".into(),
            value: code,
        }],
    };
    let outcome = runtime.execute(&call).await;
    println!(
        "{}",
        serde_json::json!({"ok": outcome.ok, "text": outcome.text})
    );
    Ok(())
}
