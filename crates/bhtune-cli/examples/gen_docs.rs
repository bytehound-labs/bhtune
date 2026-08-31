//! Regenerates every mechanically-derived CLI reference artifact from the single source of
//! truth -- the `clap` definitions in `bhtune_cli::args::Cli` -- so none of it can silently
//! drift from what `bhtune` actually accepts, the way hand-written usage docs would. Run via:
//!
//! ```sh
//! cargo run -p bhtune-cli --example gen_docs --features schemars
//! ```
//!
//! Regenerates:
//! - `docs/reference/cli.md` -- the full CLI reference, one page (clap-markdown).
//! - `man/*.1` -- one man page per command and subcommand, git-style naming
//!   (`bhtune-template-list.1`, etc.), so `pkg-aur` has real pages to install into
//!   `/usr/share/man/man1/` (clap_mangen).
//! - `completions/bhtune.bash`, `completions/_bhtune`, `completions/bhtune.fish` (clap_complete).
//! - `docs/reference/config.md` -- JSON Schema for `bhtune.toml` (`BhtuneConfig`) and the
//!   DCS/PLC template catalog shape (`DcsTemplate`), via `schemars`.
//!
//! CI (`checks.yml`) runs this and then `git diff --exit-code` against every path above: if
//! the committed output doesn't match what today's code produces, the build fails -- the same
//! regenerate-and-diff pattern `bhtune-server`'s `gen_openapi` example already established for
//! `openapi.json` (see that file's doc comment, which names this example as the intended
//! reuse of the pattern).

use clap::CommandFactory;
use std::path::{Path, PathBuf};

fn main() {
    let root = workspace_root();
    generate_cli_reference(&root);
    generate_man_pages(&root);
    generate_completions(&root);
    generate_config_schema(&root);
}

/// `CARGO_MANIFEST_DIR` is this crate's own directory (`crates/bhtune-cli`); the workspace
/// root is two levels up. Mirrors `bhtune-server/examples/gen_openapi.rs`'s identical helper,
/// deliberately not relying on the process's current working directory, since `cargo run
/// --example` does not change it to the crate's own directory.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/bhtune-cli must be nested two levels under the workspace root")
        .to_path_buf()
}

fn generate_cli_reference(root: &Path) {
    let options = clap_markdown::MarkdownOptions::new()
        .title("bhtune CLI reference".to_string())
        .show_footer(false);
    let markdown = clap_markdown::help_markdown_custom::<bhtune_cli::args::Cli>(&options);
    write(&root.join("docs/reference/cli.md"), markdown);
}

/// One man page per command/subcommand (`bhtune.1`, `bhtune-tune.1`,
/// `bhtune-template.1`, `bhtune-template-list.1`, ...), matching the convention real
/// multi-command tools like git and cargo use -- a single page covering every nested
/// subcommand in full doesn't fit the one-page-per-topic structure `man` itself assumes.
fn generate_man_pages(root: &Path) {
    let dir = root.join("man");
    let cmd = bhtune_cli::args::Cli::command();
    write_man_page_recursive(&dir, cmd, "bhtune");
}

fn write_man_page_recursive(dir: &Path, cmd: clap::Command, name: &str) {
    // `Command::name` needs `impl Into<clap::builder::Str>`, which only accepts an owned
    // `String` behind clap's `string` feature (unused elsewhere in this crate, not worth
    // enabling workspace-wide for one codegen example) -- leaking is fine here since this
    // binary generates a handful of short-lived strings and exits immediately after.
    let cmd = cmd.name(&*Box::leak(name.to_string().into_boxed_str()));
    let subcommands: Vec<clap::Command> = cmd.get_subcommands().cloned().collect();

    let mut buffer: Vec<u8> = Vec::new();
    clap_mangen::Man::new(cmd)
        .render(&mut buffer)
        .expect("rendering a valid clap::Command to roff never fails");
    let roff = String::from_utf8(buffer)
        .expect("clap_mangen always emits valid UTF-8 roff")
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n");
    write(&dir.join(format!("{name}.1")), roff);

    for sub in subcommands {
        let sub_name = format!("{name}-{}", sub.get_name());
        write_man_page_recursive(dir, sub, &sub_name);
    }
}

fn generate_completions(root: &Path) {
    let dir = root.join("completions");
    let bin_name = "bhtune";

    for (shell, file_name) in [
        (clap_complete::Shell::Bash, "bhtune.bash"),
        (clap_complete::Shell::Zsh, "_bhtune"),
        (clap_complete::Shell::Fish, "bhtune.fish"),
    ] {
        let mut cmd = bhtune_cli::args::Cli::command();
        let mut buffer: Vec<u8> = Vec::new();
        clap_complete::generate(shell, &mut cmd, bin_name, &mut buffer);
        write(
            &dir.join(file_name),
            String::from_utf8(buffer).expect("clap_complete always emits valid UTF-8"),
        );
    }
}

fn generate_config_schema(root: &Path) {
    let config_schema = schemars::schema_for!(bhtune_cli::config::BhtuneConfig);
    let template_schema = schemars::schema_for!(bhtune_core::template::DcsTemplate);

    let config_json = serde_json::to_string_pretty(&config_schema)
        .expect("a generated JSON Schema always serializes");
    let template_json = serde_json::to_string_pretty(&template_schema)
        .expect("a generated JSON Schema always serializes");

    let doc = format!(
        "# Configuration reference\n\n\
        Generated from the real `BhtuneConfig`/`DcsTemplate` Rust types (`schemars`) -- \
        never hand-edit this file, run `cargo run -p bhtune-cli --example gen_docs \
        --features schemars` instead. See `docs/dcs-templates.md` for a worked, prose \
        explanation of the template fields; this page is the exhaustive machine-checked \
        contract both formats must satisfy.\n\n\
        ## `bhtune.toml`\n\n\
        JSON Schema for bhtune's TOML config file (`crate::config::BhtuneConfig` in \
        `bhtune-cli`). Every field is optional -- see `AGENTS.md`'s `cli-config` notes for \
        the full `CLI flag > env var > TOML config file > built-in default` precedence each \
        one resolves through.\n\n\
        ```json\n\
        {config_json}\n\
        ```\n\n\
        ## DCS/PLC template catalog\n\n\
        JSON Schema for one entry in a DCS/PLC template catalog TOML file \
        (`bhtune_core::template::DcsTemplate`), the shape `bhtune template import` and the \
        embedded/user catalogs both parse. See `docs/dcs-templates.md` for a worked example \
        and contribution guidance.\n\n\
        ```json\n\
        {template_json}\n\
        ```\n"
    );
    write(&root.join("docs/reference/config.md"), doc);
}

fn write(path: &Path, contents: String) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("failed to create {}: {err}", parent.display()));
    }
    std::fs::write(path, contents)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", path.display()));
    println!("wrote {}", path.display());
}
