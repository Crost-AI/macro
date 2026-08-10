fn main() {
    match turso_core_wasm_spike::exercise_required_sql() {
        Ok(summary) => println!("{summary:?}"),
        Err(error) => {
            eprintln!("SQL spike failed: {error}");
            std::process::exit(1);
        }
    }
}
