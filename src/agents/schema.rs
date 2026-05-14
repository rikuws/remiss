use once_cell::sync::Lazy;
use serde_json::Value;

pub const TOUR_OUTPUT_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "summary": { "type": "string" },
    "reviewFocus": { "type": "string" },
    "openQuestions": {
      "type": "array",
      "items": { "type": "string" }
    },
    "warnings": {
      "type": "array",
      "items": { "type": "string" }
    },
    "overview": {
      "type": "object",
      "properties": {
        "title": { "type": "string" },
        "summary": { "type": "string" },
        "detail": { "type": "string" },
        "badge": { "type": "string" }
      },
      "required": ["title", "summary", "detail", "badge"],
      "additionalProperties": false
    },
    "steps": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "sourceStepId": { "type": "string" },
          "title": { "type": "string" },
          "summary": { "type": "string" },
          "detail": { "type": "string" },
          "badge": { "type": "string" }
        },
        "required": ["sourceStepId", "title", "summary", "detail", "badge"],
        "additionalProperties": false
      }
    },
    "sections": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "title": { "type": "string" },
          "summary": { "type": "string" },
          "detail": { "type": "string" },
          "badge": { "type": "string" },
          "category": {
            "type": "string",
            "enum": [
              "auth-security",
              "data-state",
              "api-io",
              "ui-ux",
              "tests",
              "docs",
              "config",
              "infra",
              "refactor",
              "performance",
              "reliability",
              "other"
            ]
          },
          "priority": {
            "type": "string",
            "enum": ["low", "medium", "high"]
          },
          "stepIds": {
            "type": "array",
            "items": { "type": "string" }
          },
          "reviewPoints": {
            "type": "array",
            "items": { "type": "string" }
          },
          "callsites": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "title": { "type": "string" },
                "path": { "type": "string" },
                "line": { "type": ["integer", "null"] },
                "summary": { "type": "string" },
                "snippet": { "type": ["string", "null"] }
              },
              "required": ["title", "path", "line", "summary", "snippet"],
              "additionalProperties": false
            }
          }
        },
        "required": [
          "title",
          "summary",
          "detail",
          "badge",
          "category",
          "priority",
          "stepIds",
          "reviewPoints",
          "callsites"
        ],
        "additionalProperties": false
      }
    }
  },
  "required": [
    "summary",
    "reviewFocus",
    "openQuestions",
    "warnings",
    "overview",
    "steps",
    "sections"
  ],
  "additionalProperties": false
}"#;

pub const REVIEW_BRIEF_OUTPUT_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "confidence": {
      "type": "string",
      "enum": ["low", "medium", "high"]
    },
    "briefParagraph": {
      "type": "string",
      "minLength": 1,
      "maxLength": 280
    },
    "likelyIntent": {
      "type": "string",
      "minLength": 1,
      "maxLength": 120
    },
    "changedSummary": {
      "type": "array",
      "items": {
        "type": "string",
        "minLength": 1,
        "maxLength": 100
      },
      "minItems": 1,
      "maxItems": 2
    },
    "risksQuestions": {
      "type": "array",
      "items": {
        "type": "string",
        "minLength": 1,
        "maxLength": 100
      },
      "minItems": 1,
      "maxItems": 1
    },
    "warnings": {
      "type": "array",
      "items": {
        "type": "string",
        "maxLength": 100
      },
      "maxItems": 1
    },
    "relatedFilePaths": {
      "type": "array",
      "items": { "type": "string" }
    }
  },
  "required": [
    "confidence",
    "briefParagraph",
    "likelyIntent",
    "changedSummary",
    "risksQuestions",
    "warnings",
    "relatedFilePaths"
  ],
  "additionalProperties": false
}"#;

#[allow(dead_code)]
pub static TOUR_OUTPUT_SCHEMA_VALUE: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(TOUR_OUTPUT_SCHEMA_JSON)
        .expect("TOUR_OUTPUT_SCHEMA_JSON must be valid JSON")
});

#[allow(dead_code)]
pub static REVIEW_BRIEF_OUTPUT_SCHEMA_VALUE: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(REVIEW_BRIEF_OUTPUT_SCHEMA_JSON)
        .expect("REVIEW_BRIEF_OUTPUT_SCHEMA_JSON must be valid JSON")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_parses() {
        let value = &*TOUR_OUTPUT_SCHEMA_VALUE;
        assert_eq!(value["type"], "object");
        assert!(value["properties"]["overview"].is_object());
        let section = &value["properties"]["sections"]["items"];
        assert!(section["properties"]["category"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("auth-security")));
        assert!(section["properties"]["priority"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("high")));
        assert!(section["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("category")));
        assert!(section["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("priority")));
    }

    #[test]
    fn review_brief_schema_parses() {
        let value = &*REVIEW_BRIEF_OUTPUT_SCHEMA_VALUE;
        assert_eq!(value["type"], "object");
        assert!(value["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("briefParagraph")));
        assert_eq!(
            value["properties"]["briefParagraph"]["maxLength"].as_u64(),
            Some(280)
        );
        assert_eq!(
            value["properties"]["likelyIntent"]["maxLength"].as_u64(),
            Some(120)
        );
        assert_eq!(
            value["properties"]["changedSummary"]["maxItems"].as_u64(),
            Some(2)
        );
        assert_eq!(
            value["properties"]["changedSummary"]["items"]["maxLength"].as_u64(),
            Some(100)
        );
        assert_eq!(
            value["properties"]["risksQuestions"]["maxItems"].as_u64(),
            Some(1)
        );
        assert_eq!(
            value["properties"]["warnings"]["maxItems"].as_u64(),
            Some(1)
        );
        assert!(value["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("likelyIntent")));
        assert!(value["properties"]["confidence"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("high")));
    }
}
