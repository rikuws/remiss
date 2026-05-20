use once_cell::sync::Lazy;
use serde_json::Value;

pub const REVIEW_BRIEF_OUTPUT_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "confidence": {
      "type": "string",
      "enum": ["low", "medium", "high"]
    },
    "readingMode": {
      "type": "string",
      "enum": ["scan"]
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
    "nextBestReadingAction": {
      "type": "string",
      "minLength": 1,
      "maxLength": 160
    },
    "confidenceReason": {
      "type": "string",
      "minLength": 1,
      "maxLength": 160
    },
    "understandingWarnings": {
      "type": "array",
      "items": {
        "type": "string",
        "maxLength": 140
      },
      "maxItems": 3
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
    "readingMode",
    "briefParagraph",
    "likelyIntent",
    "changedSummary",
    "risksQuestions",
    "nextBestReadingAction",
    "confidenceReason",
    "understandingWarnings",
    "warnings",
    "relatedFilePaths"
  ],
  "additionalProperties": false
}"#;

#[allow(dead_code)]
pub static REVIEW_BRIEF_OUTPUT_SCHEMA_VALUE: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(REVIEW_BRIEF_OUTPUT_SCHEMA_JSON)
        .expect("REVIEW_BRIEF_OUTPUT_SCHEMA_JSON must be valid JSON")
});

#[cfg(test)]
mod tests {
    use super::*;

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
            value["properties"]["nextBestReadingAction"]["maxLength"].as_u64(),
            Some(160)
        );
        assert_eq!(
            value["properties"]["understandingWarnings"]["maxItems"].as_u64(),
            Some(3)
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
        assert!(value["properties"]["readingMode"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("scan")));
    }
}
