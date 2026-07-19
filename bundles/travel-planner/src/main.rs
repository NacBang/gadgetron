use std::os::fd::FromRawFd;

use gadgetron_bundle_runtime::BundleBrokerClient;
use gadgetron_bundle_travel_planner::TravelPlannerRuntime;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let digest = std::env::var("GADGETRON_BUNDLE_MANIFEST_SHA256").unwrap_or_else(|_| {
        eprintln!("GADGETRON_BUNDLE_MANIFEST_SHA256 is required");
        std::process::exit(2);
    });
    let runtime = TravelPlannerRuntime::new(digest).unwrap_or_else(|error| {
        eprintln!("cannot initialize Travel Planner runtime: {error}");
        std::process::exit(2);
    });
    match std::env::var("GADGETRON_BUNDLE_BROKER_FD")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
    {
        Some(3) => {}
        _ => {
            eprintln!("GADGETRON_BUNDLE_BROKER_FD must identify fixed fd 3");
            std::process::exit(2);
        }
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
        eprintln!("Travel Planner runtime stopped: {error}");
        std::process::exit(1);
    }
}
