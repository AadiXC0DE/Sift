use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(rename = "emailAddress")]
    pub email_address: String,
    #[serde(rename = "messagesTotal")]
    pub messages_total: i64,
    #[serde(rename = "historyId")]
    pub history_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub color: Option<LabelColor>,
    #[serde(rename = "labelListVisibility")]
    pub list_visibility: Option<String>,
    #[serde(rename = "messageListVisibility")]
    pub msg_visibility: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelColor {
    #[serde(rename = "backgroundColor")]
    pub bg: Option<String>,
    #[serde(rename = "textColor")]
    pub fg: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListMessagesResponse {
    pub messages: Option<Vec<IdPair>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "resultSizeEstimate")]
    pub size: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdPair {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "labelIds")]
    pub label_ids: Option<Vec<String>>,
    pub snippet: Option<String>,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
    #[serde(rename = "internalDate")]
    pub internal_date: Option<String>,
    #[serde(rename = "sizeEstimate")]
    pub size_estimate: Option<i64>,
    pub payload: Option<MessagePart>,
    pub raw: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePart {
    #[serde(rename = "partId")]
    pub part_id: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    pub headers: Option<Vec<Header>>,
    pub body: Option<Body>,
    pub parts: Option<Vec<MessagePart>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Body {
    #[serde(rename = "attachmentId")]
    pub attachment_id: Option<String>,
    pub size: Option<i64>,
    pub data: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryResponse {
    pub history: Option<Vec<HistoryRec>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "historyId")]
    pub history_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRec {
    pub id: String,
    #[serde(rename = "messagesAdded")]
    pub added: Option<Vec<AddedDel>>,
    #[serde(rename = "messagesDeleted")]
    pub deleted: Option<Vec<AddedDel>>,
    #[serde(rename = "labelsAdded")]
    pub labels_added: Option<Vec<LabelChg>>,
    #[serde(rename = "labelsRemoved")]
    pub labels_removed: Option<Vec<LabelChg>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddedDel {
    pub message: IdPair,
    #[serde(rename = "labelIds")]
    pub label_ids: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelChg {
    pub message: IdPair,
    #[serde(rename = "labelIds")]
    pub label_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifyRequest {
    #[serde(rename = "addLabelIds")]
    pub add: Vec<String>,
    #[serde(rename = "removeLabelIds")]
    pub remove: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchModifyRequest {
    pub ids: Vec<String>,
    #[serde(rename = "addLabelIds")]
    pub add: Vec<String>,
    #[serde(rename = "removeLabelIds")]
    pub remove: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub raw: String,
    #[serde(rename = "threadId")]
    pub thread_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftMsg {
    pub id: String,
    pub message: Message,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Userinfo {
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub data: Option<String>,
    pub size: Option<i64>,
}
