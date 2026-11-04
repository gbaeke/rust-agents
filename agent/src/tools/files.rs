//! A scratch filesystem that lives in the session, not on disk. The agent
//! uses it to keep notes and drafts out of its context window, and the user
//! sees every file in the UI.

use rig::tool::{Tool, ToolContext};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{SharedWorkspace, ToolError, schema};

const MAX_FILE_BYTES: usize = 200_000;

fn normalize(path: &str) -> Result<String, ToolError> {
    let path = path.trim().trim_start_matches("./").trim_start_matches('/');
    if path.is_empty() || path.split('/').any(|part| part == ".." || part.is_empty()) {
        return Err(ToolError(format!("invalid path `{path}`; use a relative path like notes/plan.md")));
    }
    Ok(path.to_owned())
}

#[derive(Deserialize, JsonSchema)]
pub struct NoArgs {}

pub struct ListFiles {
    pub workspace: SharedWorkspace,
}

impl Tool for ListFiles {
    const NAME: &'static str = "ls";
    type Error = ToolError;
    type Args = NoArgs;
    type Output = String;

    fn description(&self) -> String {
        "List the files in your scratch workspace with their sizes.".to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<NoArgs>()
    }

    async fn call(&self, _: &mut ToolContext, _: NoArgs) -> Result<String, ToolError> {
        let workspace = self.workspace.lock();
        if workspace.files.is_empty() {
            return Ok("The workspace is empty.".to_owned());
        }
        Ok(workspace
            .files
            .iter()
            .map(|(path, content)| format!("{path} ({} bytes)", content.len()))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub path: String,
    /// First line to return, 1-based. Defaults to 1.
    pub offset: Option<usize>,
    /// Maximum number of lines to return. Defaults to 500.
    pub limit: Option<usize>,
}

pub struct ReadFile {
    pub workspace: SharedWorkspace,
}

impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    type Error = ToolError;
    type Args = ReadFileArgs;
    type Output = String;

    fn description(&self) -> String {
        "Read a file from your scratch workspace, with line numbers.".to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<ReadFileArgs>()
    }

    async fn call(&self, _: &mut ToolContext, args: ReadFileArgs) -> Result<String, ToolError> {
        let path = normalize(&args.path)?;
        let workspace = self.workspace.lock();
        let content = workspace
            .files
            .get(&path)
            .ok_or_else(|| ToolError(format!("no file `{path}`; call ls to see the files")))?;
        let offset = args.offset.unwrap_or(1).max(1);
        let lines: Vec<String> = content
            .lines()
            .enumerate()
            .skip(offset - 1)
            .take(args.limit.unwrap_or(500))
            .map(|(index, line)| format!("{:>5}  {line}", index + 1))
            .collect();
        Ok(if lines.is_empty() { "(no lines in that range)".to_owned() } else { lines.join("\n") })
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// Relative path, e.g. `report.md` or `notes/findings.md`.
    pub path: String,
    /// The whole file content.
    pub content: String,
}

pub struct WriteFile {
    pub workspace: SharedWorkspace,
}

impl Tool for WriteFile {
    const NAME: &'static str = "write_file";
    type Error = ToolError;
    type Args = WriteFileArgs;
    type Output = String;

    fn description(&self) -> String {
        "Create or overwrite a file in your scratch workspace. Use it for notes, intermediate \
         findings and final deliverables such as reports."
            .to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<WriteFileArgs>()
    }

    async fn call(&self, _: &mut ToolContext, args: WriteFileArgs) -> Result<String, ToolError> {
        let path = normalize(&args.path)?;
        if args.content.len() > MAX_FILE_BYTES {
            return Err(ToolError(format!("files are limited to {MAX_FILE_BYTES} bytes")));
        }
        let bytes = args.content.len();
        self.workspace.lock().files.insert(path.clone(), args.content);
        Ok(format!("Wrote {bytes} bytes to {path}."))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct EditFileArgs {
    pub path: String,
    /// Exact text to replace. It must occur exactly once unless replace_all is true.
    pub old_string: String,
    pub new_string: String,
    /// Replace every occurrence instead of exactly one.
    #[serde(default)]
    pub replace_all: bool,
}

pub struct EditFile {
    pub workspace: SharedWorkspace,
}

impl Tool for EditFile {
    const NAME: &'static str = "edit_file";
    type Error = ToolError;
    type Args = EditFileArgs;
    type Output = String;

    fn description(&self) -> String {
        "Replace exact text in a workspace file. Prefer this over rewriting a whole file.".to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<EditFileArgs>()
    }

    async fn call(&self, _: &mut ToolContext, args: EditFileArgs) -> Result<String, ToolError> {
        let path = normalize(&args.path)?;
        let mut workspace = self.workspace.lock();
        let content = workspace
            .files
            .get_mut(&path)
            .ok_or_else(|| ToolError(format!("no file `{path}`")))?;
        let count = content.matches(&args.old_string).count();
        match (count, args.replace_all) {
            (0, _) => Err(ToolError("old_string was not found; read the file and copy the text exactly".to_owned())),
            (1, _) | (_, true) => {
                *content = content.replace(&args.old_string, &args.new_string);
                Ok(format!("Replaced {count} occurrence(s) in {path}."))
            }
            (n, false) => Err(ToolError(format!(
                "old_string occurs {n} times; add surrounding text to make it unique, or set replace_all"
            ))),
        }
    }
}
