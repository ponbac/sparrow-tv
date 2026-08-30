#[tokio::main]
async fn main() -> Result<(), sparrow_server::StartupError> {
    sparrow_server::run().await
}
