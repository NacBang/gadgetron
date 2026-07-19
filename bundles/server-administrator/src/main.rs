use std::os::fd::FromRawFd;

use gadgetron_bundle_server_administrator::{BundleBrokerClient, ServerAdministratorRuntime};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let digest = match std::env::var("GADGETRON_BUNDLE_MANIFEST_SHA256") {
        Ok(value) => value,
        Err(_) => {
            eprintln!("GADGETRON_BUNDLE_MANIFEST_SHA256 is required");
            std::process::exit(2);
        }
    };
    let runtime = match ServerAdministratorRuntime::new(digest) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot initialize Server Administrator runtime: {error}");
            std::process::exit(2);
        }
    };
    let broker_fd = match std::env::var("GADGETRON_BUNDLE_BROKER_FD")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
    {
        Some(3) => 3,
        _ => {
            eprintln!("GADGETRON_BUNDLE_BROKER_FD must identify fixed fd 3");
            std::process::exit(2);
        }
    };
    // SAFETY: the Core supervisor transfers ownership of exactly this Unix
    // socket descriptor to the runtime process.
    let broker = unsafe { std::os::unix::net::UnixStream::from_raw_fd(broker_fd) };
    if let Err(error) = broker.set_nonblocking(true) {
        eprintln!("cannot configure Bundle broker channel: {error}");
        std::process::exit(2);
    }
    let broker = match tokio::net::UnixStream::from_std(broker) {
        Ok(broker) => broker,
        Err(error) => {
            eprintln!("cannot attach Bundle broker channel: {error}");
            std::process::exit(2);
        }
    };
    let client = BundleBrokerClient::attach(broker, runtime.identity().clone());
    let mut runtime = runtime.with_broker(client);
    if let Err(error) = runtime.serve(tokio::io::stdin(), tokio::io::stdout()).await {
        eprintln!("Server Administrator runtime stopped: {error}");
        std::process::exit(1);
    }
}
