use std::os::fd::FromRawFd;

use gadgetron_bundle_restaurant_research::RestaurantResearchRuntime;
use gadgetron_bundle_runtime::BundleBrokerClient;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let digest = required_env("GADGETRON_BUNDLE_MANIFEST_SHA256");
    let runtime = RestaurantResearchRuntime::new(digest).unwrap_or_else(|error| {
        eprintln!("cannot initialize Restaurant Research runtime: {error}");
        std::process::exit(2);
    });
    if std::env::var("GADGETRON_BUNDLE_BROKER_FD")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        != Some(3)
    {
        eprintln!("GADGETRON_BUNDLE_BROKER_FD must identify fixed fd 3");
        std::process::exit(2);
    }
    // SAFETY: Core transfers ownership of exactly fixed fd 3 to this process.
    let broker = unsafe { std::os::unix::net::UnixStream::from_raw_fd(3) };
    broker.set_nonblocking(true).unwrap_or_else(|error| {
        eprintln!("cannot configure Bundle broker channel: {error}");
        std::process::exit(2);
    });
    let broker = tokio::net::UnixStream::from_std(broker).unwrap_or_else(|error| {
        eprintln!("cannot attach Bundle broker channel: {error}");
        std::process::exit(2);
    });
    let client = BundleBrokerClient::attach(broker, runtime.identity().clone());
    let mut runtime = runtime.with_broker(client);
    if let Err(error) = runtime.serve(tokio::io::stdin(), tokio::io::stdout()).await {
        eprintln!("Restaurant Research runtime stopped: {error}");
        std::process::exit(1);
    }
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("{name} is required");
        std::process::exit(2);
    })
}
