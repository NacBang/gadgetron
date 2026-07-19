fn main() {
    let mut args = std::env::args();
    let _program = args.next();
    let marker = args.next();
    let spec = args.next();
    if marker.as_deref() != Some(gadgetron_bundle_supervisor::INTERNAL_HELPER_MARKER)
        || spec.is_none()
        || args.next().is_some()
    {
        eprintln!("invalid internal Bundle sandbox helper invocation");
        std::process::exit(2);
    }
    if let Err(error) = gadgetron_bundle_supervisor::run_internal_helper(
        spec.as_deref().expect("validated helper spec argument"),
    ) {
        eprintln!("Bundle sandbox helper failed: {error}");
        std::process::exit(1);
    }
}
