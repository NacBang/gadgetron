//! Bundle size budget: the embedded `WEB_DIST` total must stay under 3 MB.
//!
//! Design: `docs/design/phase2/03-gadgetron-web.md` §22.
//! Review cross-ref: QA-W-B5 (shiki regression path).
//!
//! During bootstrap (before task #4 scaffolds `web/`), `WEB_DIST` contains only
//! the fallback `index.html` which is well under budget. After task #4 the budget
//! gates regression — if shiki grammar set changes or Next.js bundle grows past
//! 3 MB total, this test fails with the exact byte count, and the design doc §24
//! bullet 2 must be revisited.

const BUDGET_BYTES: u64 = 3 * 1024 * 1024; // 3 MB

#[test]
fn web_dist_total_bytes_under_budget() {
    // We can't directly import the `WEB_DIST` static (it's a private static in
    // gadgetron-web::lib), so we walk the on-disk `web/dist/` directory which is
    // what `include_dir!` embeds. This is the same source of truth as the compiled
    // binary — if build.rs generates it, this test measures it.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dist = std::path::Path::new(manifest_dir).join("web").join("dist");
    assert!(
        dist.exists(),
        "web/dist/ must exist after build.rs runs; got missing path {dist:?}"
    );
    let total = dir_total_bytes(&dist).unwrap();
    assert!(
        total <= BUDGET_BYTES,
        "WEB_DIST total is {total} bytes, exceeds {BUDGET_BYTES} budget. \
         Did shiki grammar set change? See docs/design/phase2/03-gadgetron-web.md §24 bullet 2."
    );
}

fn dir_total_bytes(dir: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            total += dir_total_bytes(&entry.path())?;
        } else if ty.is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}
