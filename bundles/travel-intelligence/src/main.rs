use gadgetron_bundle_runtime::ManifestBundleRuntime;
use gadgetron_bundle_sdk::{BundleId, BundleRuntimeIdentity};
use semver::Version;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let digest = required_env("GADGETRON_BUNDLE_MANIFEST_SHA256");
    let identity = BundleRuntimeIdentity::new(
        BundleId::new("travel-intelligence").expect("static Bundle id is valid"),
        Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is valid semver"),
    );
    let mut runtime = ManifestBundleRuntime::new(
        identity,
        digest,
        "Travel Intelligence knowledge contracts are ready",
    )
    .unwrap_or_else(|error| {
        eprintln!("cannot initialize Travel Intelligence runtime: {error}");
        std::process::exit(2);
    });
    if let Err(error) = runtime.serve(tokio::io::stdin(), tokio::io::stdout()).await {
        eprintln!("Travel Intelligence runtime stopped: {error}");
        std::process::exit(1);
    }
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("{name} is required");
        std::process::exit(2);
    })
}
