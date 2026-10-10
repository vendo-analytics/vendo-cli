//! `vendo version` (VE-3893) and `vendo --version`/`-V`: the CLI's version, alone on a line. With
//! `--json`, `{"version": "<version>"}`.

use serde_json::json;

use crate::output::print_json;

pub fn run(json: bool) {
    if json {
        print_json(&json!({ "version": env!("CARGO_PKG_VERSION") }));
    } else {
        println!("{}", env!("CARGO_PKG_VERSION"));
    }
}
