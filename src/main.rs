#[tokio::main]
async fn main() -> togi::error::Result<()> {
    togi::app::run().await
}
