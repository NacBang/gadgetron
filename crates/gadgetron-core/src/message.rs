use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
    /// Reasoning trace from SGLang GLM-5.1 style models (non-streaming path).
    /// Absent in standard OpenAI/Anthropic responses; `serde(default)` makes it None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Content {
    pub fn text(&self) -> Option<&str> {
        match self {
            Content::Text(s) => Some(s),
            Content::Parts(parts) => parts.iter().find_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            }),
        }
    }
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Content::Text(text.into()),
            reasoning_content: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Content::Text(text.into()),
            reasoning_content: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Content::Text(text.into()),
            reasoning_content: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_reasoning_content_deserializes() {
        let json = r#"{"role":"assistant","content":"done","reasoning_content":"step1"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.reasoning_content, Some("step1".to_string()));
    }

    #[test]
    fn message_missing_reasoning_content_is_none() {
        let json = r#"{"role":"user","content":"hi"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.reasoning_content, None);
    }

    #[test]
    fn message_constructors_set_reasoning_content_none() {
        assert_eq!(Message::user("hi").reasoning_content, None);
        assert_eq!(Message::system("sys").reasoning_content, None);
        assert_eq!(Message::assistant("ok").reasoning_content, None);
    }
}
