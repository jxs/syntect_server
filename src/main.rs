use std::env;
use std::panic;
use std::path::Path;

use lazy_static::lazy_static;
use serde::Deserialize;
use serde_json::{json, Value};
use syntect::{
    highlighting::ThemeSet,
    html::{highlighted_html_for_string, ClassStyle},
    parsing::SyntaxSet,
};
use tracing_subscriber::fmt::format::FmtSpan;
use warp::{reply::Json, Filter, Rejection};

mod css_table;
use css_table::ClassedTableGenerator;

thread_local! {
    static SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
}

lazy_static! {
    static ref THEME_SET: ThemeSet = ThemeSet::load_defaults();
}

#[derive(Debug, Deserialize)]
struct Query {
    // Deprecated field with a default empty string value, kept for backwards
    // compatability with old clients.
    #[serde(default)]
    extension: String,

    // default empty string value for backwards compat with clients who do not specify this field.
    #[serde(default)]
    filepath: String,

    // If css is set, the highlighted code will be returned as a HTML table with CSS classes
    // annotating the highlighted types.
    #[serde(default)]
    css: bool,

    // line_length_limit is ignored if css is false
    line_length_limit: Option<usize>,

    // theme is ignored if css is true
    theme: String,

    code: String,
}

async fn highlight_handler(q: Query) -> Result<Json, Rejection> {
    tracing::info!(extension = %q.extension, filepath = %q.filepath, code_len = q.code.len());
    // TODO(slimsag): In an ideal world we wouldn't be relying on catch_unwind
    // and instead Syntect would return Result types when failures occur. This
    // will require some non-trivial work upstream:
    // https://github.com/trishume/syntect/issues/98
    // tracing::info!("highlight", "query = {:?}", q);
    let result = panic::catch_unwind(|| highlight(&q));
    match result {
        Err(err) => {
            tracing::error!(backtrace = ?err);
            Ok(warp::reply::json(
                &json!({"error": "panic while highlighting code", "code": "panic"}),
            ))
        }
        Ok(v) => Ok(warp::reply::json(&v)),
    }
}

fn highlight(q: &Query) -> Value {
    panic!("in the streets of london");
    SYNTAX_SET.with(|syntax_set| {
        // Determine syntax definition by extension.
        // panic!("cenas");
        let mut is_plaintext = false;
        let syntax_def = if q.filepath.is_empty() {
            // Legacy codepath, kept for backwards-compatability with old clients.
            match syntax_set.find_syntax_by_extension(&q.extension) {
                Some(v) => v,
                None =>
                // Fall back: Determine syntax definition by first line.
                {
                    match syntax_set.find_syntax_by_first_line(&q.code) {
                        Some(v) => v,
                        None => return json!({"error": "invalid extension"}),
                    }
                }
            }
        } else {
            // Split the input path ("foo/myfile.go") into file name
            // ("myfile.go") and extension ("go").
            let path = Path::new(&q.filepath);
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("");

            // To determine the syntax definition, we must first check using the
            // filename as some syntaxes match an "extension" that is actually a
            // whole file name (e.g. "Dockerfile" or "CMakeLists.txt"); see e.g. https://github.com/trishume/syntect/pull/170
            //
            // After that, if we do not find any syntax, we can actually check by
            // extension and lastly via the first line of the code.

            // First try to find a syntax whose "extension" matches our file
            // name. This is done due to some syntaxes matching an "extension"
            // that is actually a whole file name (e.g. "Dockerfile" or "CMakeLists.txt")
            // see https://github.com/trishume/syntect/pull/170
            syntax_set
                .find_syntax_by_extension(file_name)
                .or_else(|| syntax_set.find_syntax_by_extension(extension))
                .or_else(|| syntax_set.find_syntax_by_first_line(&q.code))
                .unwrap_or_else(|| {
                    is_plaintext = true;
                    syntax_set.find_syntax_plain_text()
                })
        };

        if q.css {
            let output = ClassedTableGenerator::new(
                syntax_set,
                syntax_def,
                &q.code,
                q.line_length_limit,
                ClassStyle::SpacedPrefixed { prefix: "hl-" },
            )
            .generate();

            json!({
                "data": output,
                "plaintext": is_plaintext,
            })
        } else {
            // TODO(slimsag): return the theme's background color (and other info??) to caller?
            // https://github.com/trishume/syntect/blob/c8b47758a3872d478c7fc740782cd468b2c0a96b/examples/synhtml.rs#L24

            // Determine theme to use.
            //
            // TODO(slimsag): We could let the query specify the theme file's actual
            // bytes? e.g. via `load_from_reader`.
            let theme = match THEME_SET.themes.get(&q.theme) {
                Some(v) => v,
                None => return json!({"error": "invalid theme", "code": "invalid_theme"}),
            };

            json!({
                "data": highlighted_html_for_string(&q.code, syntax_set, syntax_def, theme),
                "plaintext": is_plaintext,
            })
        }
    })
}

async fn handle_rejection(err: Rejection) -> Result<Json, Rejection> {
    if err.is_not_found() {
        Ok(warp::reply::json(
            &json!({"error": "resource not found", "code": "resource_not_found"}),
        ))
    } else {
        tracing::error!(reason = ?err);
        Err(err)
    }
}

fn list_features() {
    // List embedded themes.
    println!("## Embedded themes:");
    println!();
    for t in THEME_SET.themes.keys() {
        println!("- `{}`", t);
    }
    println!();

    // List supported file extensions.
    SYNTAX_SET.with(|syntax_set| {
        println!("## Supported file extensions:");
        println!();
        for sd in syntax_set.syntaxes() {
            println!("- {} (`{}`)", sd.name, sd.file_extensions.join("`, `"));
        }
        println!();
    });
}

#[tokio::main]
async fn main() {
    let filter = env::var("RUST_LOG").unwrap_or_else(|_| "info,warp=debug".to_owned());
    match env::var("QUIET") {
        Ok(v) => {
            if v != "true" {
                list_features()
            }
        }
        Err(_) => list_features(),
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let highlight = warp::path::end()
        .and(warp::post())
        .and(warp::body::json())
        .and_then(highlight_handler)
        .with(warp::trace::named("highlight"));

    let health = warp::path!("health")
        .map(|| "OK")
        .with(warp::trace::named("health"));

    let routes = highlight
        .or(health)
        .recover(handle_rejection)
        .with(warp::trace::request());
    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}
