use glyphlow::AppSignal;
use glyphlow::ax_element::Target;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = "/Users/farseerhe/.cache/glyphlow/glyphlow.socket";
    let mut stream = UnixStream::connect(socket_path).await?;
    println!("Connected to {}", socket_path);

    let signal = AppSignal::Activate(Target::Clickable);

    let mut payload = serde_json::to_string(&signal)?;
    payload.push('\n');

    // 5. Send over the socket stream
    stream.write_all(payload.as_bytes()).await?;
    stream.flush().await?;

    println!("Successfully sent: {:?}", signal);

    Ok(())
}
