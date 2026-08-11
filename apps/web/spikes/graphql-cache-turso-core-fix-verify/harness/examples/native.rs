fn main() {
    let report = turso_temp_fix_parent_or_head::native_report()
        .expect("native verification report must execute");
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("report must serialize")
    );
}

mod turso_temp_fix_parent_or_head {
    #[cfg(feature = "head-revision")]
    pub use turso_temp_fix_head::native_report;
    #[cfg(feature = "parent-revision")]
    pub use turso_temp_fix_parent::native_report;
}
