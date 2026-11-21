#[path = "../operator.rs"]
mod operator;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    operator::entry().await
}
