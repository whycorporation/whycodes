use super::*;
use std::path::PathBuf;
use tempfile::TempDir;

/// A notebook as Jupyter writes it: sorted keys, 1-space indent, trailing newline.
const SAMPLE: &str = r##"{
 "cells": [
  {
   "cell_type": "markdown",
   "id": "a1",
   "metadata": {},
   "source": [
    "# Title\n",
    "Some text"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": 3,
   "id": "b2",
   "metadata": {
    "tags": [
     "keep"
    ]
   },
   "outputs": [
    {
     "name": "stdout",
     "output_type": "stream",
     "text": [
      "hi\n"
     ]
    },
    {
     "data": {
      "image/png": "iVBORw0KGgo=",
      "text/plain": [
       "<Figure>"
      ]
     },
     "metadata": {},
     "output_type": "display_data"
    }
   ],
   "source": [
    "import os\n",
    "print('hi')"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": null,
   "id": "c3",
   "metadata": {},
   "outputs": [],
   "source": []
  }
 ],
 "metadata": {
  "kernelspec": {
   "display_name": "Python 3",
   "language": "python",
   "name": "python3"
  }
 },
 "nbformat": 4,
 "nbformat_minor": 5
}
"##;

const SUMMARY: &str = "# out: 2 outputs (stream, image/png, text/plain)";

const RENDERED: &str = "# %% [markdown] cell:1\n# Title\nSome text\n\n\
# %% [code] cell:2\nimport os\nprint('hi')\n# out: 2 outputs (stream, image/png, text/plain)\n\n\
# %% [code] cell:3\n\n";

fn nb(raw: &str) -> Notebook {
    Notebook::open(Path::new("n.ipynb"), raw).unwrap()
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn cells(text: &str) -> Vec<Value> {
    json(text)["cells"].as_array().unwrap().clone()
}

#[test]
fn notebook_path_by_extension() {
    assert!(is_notebook_path(Path::new("a.ipynb")));
    assert!(is_notebook_path(Path::new("A.IPYNB")));
    assert!(!is_notebook_path(Path::new("a.json")));
    assert!(!is_notebook_path(Path::new("ipynb")));
}

#[test]
fn open_rejects_non_notebooks() {
    assert!(Notebook::open(Path::new("a.json"), SAMPLE).is_none());
    assert!(Notebook::open(Path::new("a.ipynb"), "not json").is_none());
    assert!(Notebook::open(Path::new("a.ipynb"), "[1]").is_none());
    assert!(Notebook::open(Path::new("a.ipynb"), r#"{"cells": [1]}"#).is_none());
}

#[test]
fn read_missing_file_is_none() {
    let dir = TempDir::new().unwrap();
    assert!(Notebook::read(&dir.path().join("gone.ipynb")).is_none());
    std::fs::write(dir.path().join("ok.ipynb"), SAMPLE).unwrap();
    assert!(Notebook::read(&dir.path().join("ok.ipynb")).is_some());
}

#[test]
fn renders_markers_sources_and_output_summary() {
    assert_eq!(nb(SAMPLE).rendering, RENDERED);
}

#[test]
fn renders_string_sources_and_odd_outputs() {
    let raw = r#"{"cells": [
        {"cell_type": "code", "source": "x = 1\n", "outputs": [{"output_type": "error"}]},
        {"cell_type": "code", "source": 7, "outputs": [{}]},
        {"source": "plain"}
    ]}"#;
    assert_eq!(
        nb(raw).rendering,
        "# %% [code] cell:1\nx = 1\n\n# out: 1 output (error)\n\n\
         # %% [code] cell:2\n\n# out: 1 output (output)\n\n\
         # %% [raw] cell:3\nplain\n"
    );
}

#[test]
fn untouched_rendering_round_trips_byte_identical() {
    let notebook = nb(SAMPLE);
    assert_eq!(notebook.apply(&notebook.rendering).unwrap(), SAMPLE);
    // Not re-serialized either: a style the writer cannot reproduce survives.
    let odd =
        "{\"cells\":[{\"cell_type\":\"code\",\"source\":[\"x\"],\"outputs\":[]}],\"nbformat\":4}";
    let notebook = nb(odd);
    assert_eq!(notebook.apply(&notebook.rendering).unwrap(), odd);
}

#[test]
fn writer_reproduces_jupyter_style() {
    assert_eq!(serialize(&json(SAMPLE), SAMPLE), SAMPLE);
}

#[test]
fn changed_code_cell_drops_outputs_and_keeps_id_and_metadata() {
    let notebook = nb(SAMPLE);
    let edited = notebook.rendering.replace("print('hi')", "print('bye')");
    let out = notebook.apply(&edited).unwrap();
    let cell = &cells(&out)[1];
    assert_eq!(cell["source"], json!(["import os\n", "print('bye')"]));
    assert_eq!(cell["outputs"], json!([]));
    assert_eq!(cell["execution_count"], Value::Null);
    assert_eq!(cell["id"], "b2");
    assert_eq!(cell["metadata"], json!({"tags": ["keep"]}));
    // Other cells and the top level are untouched; style is kept.
    assert_eq!(cells(&out)[0], cells(SAMPLE)[0]);
    assert_eq!(json(&out)["metadata"], json(SAMPLE)["metadata"]);
    assert!(out.starts_with("{\n \"cells\": [\n  {\n"));
    assert!(out.ends_with("}\n"));
}

#[test]
fn changed_markdown_cell_keeps_string_source_form() {
    let raw = r#"{"cells": [{"cell_type": "markdown", "metadata": {}, "source": "old"}], "nbformat": 4, "nbformat_minor": 4}"#;
    let notebook = nb(raw);
    let out = notebook.apply("# %% [markdown] cell:1\nnew\n").unwrap();
    assert_eq!(cells(&out)[0]["source"], "new");
    // Compact input stays compact, without a trailing newline.
    assert!(!out.contains('\n'));
}

#[test]
fn edit_with_output_summary_removed_still_maps_back() {
    let notebook = nb(SAMPLE);
    let edited = notebook.rendering.replace(&format!("{SUMMARY}\n"), "");
    // The source is unchanged, so the cell keeps its outputs.
    assert_eq!(notebook.apply(&edited).unwrap(), SAMPLE);
}

#[test]
fn summary_text_inside_a_source_line_is_source() {
    let notebook = nb(SAMPLE);
    let edited = notebook.rendering.replace(
        &format!("print('hi')\n{SUMMARY}"),
        &format!("print('hi')x{SUMMARY}\n{SUMMARY}"),
    );
    let out = notebook.apply(&edited).unwrap();
    assert_eq!(
        cells(&out)[1]["source"],
        json!(["import os\n", format!("print('hi')x{SUMMARY}")])
    );
    // A summary line glued to the previous line is not stripped either.
    let edited = notebook.rendering.replace(
        &format!("print('hi')\n{SUMMARY}"),
        &format!("print('hi'){SUMMARY}"),
    );
    let out = notebook.apply(&edited).unwrap();
    assert_eq!(
        cells(&out)[1]["source"],
        json!(["import os\n", format!("print('hi'){SUMMARY}")])
    );
}

#[test]
fn insert_new_cells_with_and_without_ids() {
    let notebook = nb(SAMPLE);
    let edited = format!(
        "{}\n# %% [code]\nnew = 1\n\n# %% [markdown] cell:new\nnote\n",
        notebook.rendering
    );
    let out = notebook.apply(&edited).unwrap();
    let all = cells(&out);
    assert_eq!(all.len(), 5);
    assert_eq!(all[3]["source"], json!(["new = 1"]));
    assert_eq!(all[3]["outputs"], json!([]));
    assert_eq!(all[3]["execution_count"], Value::Null);
    assert_eq!(all[3]["metadata"], json!({}));
    assert_eq!(all[3]["id"].as_str().unwrap().len(), 8);
    assert_eq!(all[4]["cell_type"], "markdown");
    assert!(all[4].get("outputs").is_none());
    assert_ne!(all[3]["id"], all[4]["id"]);
    // An nbformat 4.4 notebook without ids gets none.
    let old = r#"{"cells": [], "nbformat": 4, "nbformat_minor": 4}"#;
    let out = nb(old).apply("# %% [raw]\nr\n").unwrap();
    assert!(cells(&out)[0].get("id").is_none());
}

#[test]
fn fresh_id_skips_taken_ids() {
    let mut ids = HashSet::new();
    let first = fresh_id("seed", &mut ids);
    let mut taken = HashSet::from([first.clone()]);
    let second = fresh_id("seed", &mut taken);
    assert_ne!(first, second);
}

#[test]
fn delete_and_reorder_cells() {
    let notebook = nb(SAMPLE);
    let edited = "# %% [code] cell:3\n\n\n# %% [markdown] cell:1\n# Title\nSome text\n";
    let out = notebook.apply(edited).unwrap();
    let orig = cells(SAMPLE);
    assert_eq!(cells(&out), vec![orig[2].clone(), orig[0].clone()]);
    // Every block removed: an empty notebook.
    assert_eq!(cells(&notebook.apply("").unwrap()), Vec::<Value>::new());
}

#[test]
fn change_cell_type_both_ways() {
    let notebook = nb(SAMPLE);
    let edited = notebook
        .rendering
        .replace("[markdown] cell:1", "[code] cell:1")
        .replace("[code] cell:2", "[raw] cell:2");
    let out = notebook.apply(&edited).unwrap();
    let all = cells(&out);
    assert_eq!(all[0]["cell_type"], "code");
    assert_eq!(all[0]["outputs"], json!([]));
    assert_eq!(all[1]["cell_type"], "raw");
    assert!(all[1].get("outputs").is_none());
    assert!(all[1].get("execution_count").is_none());
    // The summary still belongs to cell 2 and is not taken for source.
    assert_eq!(all[1]["source"], json!(["import os\n", "print('hi')"]));
}

#[test]
fn damaged_markers_are_refused() {
    let notebook = nb(SAMPLE);
    let dup = notebook.rendering.replace("cell:3", "cell:2");
    let e = notebook.apply(&dup).unwrap_err();
    assert!(e.contains("`cell:2` appears more than once"), "{e}");
    assert!(e.contains("Nothing was written"), "{e}");
    let e = notebook.apply("# %% [code] cell:9\nx\n").unwrap_err();
    assert!(
        e.contains("`cell:9` is not a cell of this notebook (it has 3)"),
        "{e}"
    );
    let e = notebook.apply("# %% [code] cell:x\n").unwrap_err();
    assert!(e.contains("`cell:x`"), "{e}");
    let e = notebook.apply("# %% [python] cell:1\n").unwrap_err();
    assert!(e.contains("unknown cell type `python`"), "{e}");
    let e = notebook.apply("\nstray\n# %% [code] cell:1\n").unwrap_err();
    assert!(
        e.contains("line 2: text before the first cell marker"),
        "{e}"
    );
}

#[test]
fn marker_grammar() {
    assert_eq!(marker("# %% [code] cell:4"), Some(("code", "4")));
    assert_eq!(marker("# %% [code]"), Some(("code", "")));
    assert_eq!(marker("# %% [code] extra"), None);
    assert_eq!(marker("# %% [code"), None);
    assert_eq!(marker("# %% plain comment"), None);
}

#[test]
fn source_without_trailing_separator_is_tolerated() {
    let notebook = nb(SAMPLE);
    let edited = "# %% [markdown] cell:1\n# Title\nSome text\n# %% [code] cell:3";
    let all = cells(&notebook.apply(edited).unwrap());
    assert_eq!(all[0], cells(SAMPLE)[0]);
    assert_eq!(all[1], cells(SAMPLE)[2]);
}

#[test]
fn crlf_style_is_kept() {
    let raw = SAMPLE.replace('\n', "\r\n");
    let notebook = nb(&raw);
    let edited = notebook.rendering.replace("Some text", "Other");
    let out = notebook.apply(&edited).unwrap();
    assert!(out.contains("\"Other\"\r\n"));
    assert!(out.ends_with("}\r\n"));
    assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
}

fn temp_path(dir: &TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

#[test]
fn convert_write_passes_raw_json_and_other_files_through() {
    let dir = TempDir::new().unwrap();
    let path = temp_path(&dir, "n.ipynb");
    assert_eq!(convert_write(&path, SAMPLE).unwrap(), SAMPLE);
    let text = "# %% [code] cell:1\nx\n";
    assert_eq!(convert_write(&temp_path(&dir, "a.py"), text).unwrap(), text);
}

#[test]
fn convert_write_creates_a_fresh_notebook() {
    let dir = TempDir::new().unwrap();
    let path = temp_path(&dir, "n.ipynb");
    let text = "\n# %% [markdown] cell:1\n# Hi\n\n# %% [code] cell:2\nx = 1\n";
    let out = convert_write(&path, text).unwrap();
    let value = json(&out);
    assert_eq!(value["nbformat"], 4);
    assert_eq!(value["nbformat_minor"], 5);
    assert_eq!(value["cells"][0]["source"], json!(["# Hi"]));
    assert_eq!(value["cells"][1]["source"], json!(["x = 1"]));
    assert!(out.starts_with("{\n \"cells\": [\n"));
    assert!(out.ends_with("}\n"));
    // No cells at all: the empty notebook itself.
    let empty = Notebook::fresh();
    assert_eq!(empty.apply("").unwrap(), empty.raw);
}

#[test]
fn convert_write_merges_into_the_existing_notebook() {
    let dir = TempDir::new().unwrap();
    let path = temp_path(&dir, "n.ipynb");
    std::fs::write(&path, SAMPLE).unwrap();
    assert_eq!(convert_write(&path, RENDERED).unwrap(), SAMPLE);
    let e = convert_write(&path, "# %% [code] cell:1\n\n# %% [code] cell:1\n").unwrap_err();
    assert!(e.contains("appears more than once"), "{e}");
}
