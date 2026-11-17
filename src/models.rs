use schemars::JsonSchema;
use serde::Deserialize;

/// A single page of Infinity results. Use returned cursors for subsequent calls.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// Results per page, 1–100. Defaults to 25.
    pub limit: Option<u16>,
    /// Cursor returned by the preceding response; cannot be combined with before.
    pub after: Option<String>,
    /// Cursor for the previous page; cannot be combined with after.
    pub before: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceQuery {
    /// Infinity workspace ID (as a string).
    pub workspace_id: String,
    #[serde(default)]
    pub page: Page,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardQuery {
    pub workspace_id: String,
    pub board_id: String,
    #[serde(default)]
    pub page: Page,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemsQuery {
    pub workspace_id: String,
    pub board_id: String,
    /// Optional folder ID to narrow the results.
    pub folder_id: Option<String>,
    #[serde(default)]
    pub page: Page,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemQuery {
    pub workspace_id: String,
    pub board_id: String,
    /// Infinity item ID, not a human-facing ticket number.
    pub item_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentsQuery {
    pub workspace_id: String,
    pub board_id: String,
    pub item_id: String,
    #[serde(default)]
    pub page: Page,
}
