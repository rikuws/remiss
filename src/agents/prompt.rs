use serde_json::Value;

pub fn build_stack_planning_prompt(input_json: &Value) -> String {
    let context_pretty = serde_json::to_string_pretty(input_json).expect("context must serialize");
    [
        "You are helping Remiss, a read-only pull request review IDE, create virtual review stacks.",
        "",
        "Your task is to repair and label deterministic candidate layers for one pull request.",
        "",
        "A virtual stack is not a Git branch stack. It is a local review lens. It should reconstruct the author's ideal review plan, not categorize the diff.",
        "",
        "A good stack is an ordered sequence of conceptual, independently reviewable, dependency-respecting layers.",
        "Each layer must answer one clear review question and contain at least one substantive change.",
        "Each layer must be an understanding unit: after reading it, the reviewer should have one coherent piece of the change loaded into memory.",
        "Prefer layers that separate reading modes: foundation/context, behavior change, integration impact, critical risk surface, and verification/tests.",
        "Do not create layers from superficial diff categories such as imports, whitespace, comments, or small cleanup unless the whole layer is a coherent mechanical formatting/comment-only change.",
        "",
        "Critical rules:",
        "- Do not invent atom IDs.",
        "- Do not omit atom IDs.",
        "- Assign every atom exactly once.",
        "- Every atom id from input.atoms MUST appear exactly once across all layer atom_ids and manual_review_atom_ids combined. If you are unsure where an atom belongs, put it in manual_review_atom_ids rather than dropping it.",
        "- input.structural_evidence is deterministic support, not a replacement for atom coverage. Use it when it clarifies syntactic movement, localized rewrites, or structural risk, and ignore it when it is partial or unavailable.",
        "- input.semantic_evidence is deterministic Sem support, not a replacement for atom coverage. Prefer its mapped semantic layer candidates over directory buckets when the mappings are coherent, but keep exact atom coverage authoritative.",
        "- Do not create Git branches or PRs.",
        "- Do not suggest rewriting history.",
        "- Start from candidate_layers, dependency_edges, and atom metadata. Repair them when needed; do not perform free-form clustering from raw file categories.",
        "- dependency_edges only contains symbol-reference and test-target relationships; role-based ordering (foundation/types -> core -> integration -> tests) is implicit in atoms[*].role and must be preserved without explicit edges.",
        "- Prefer semantic review order over commit boundaries when commits are too coarse.",
        "- Commits are signals, not authoritative layers.",
        "- If a PR has only 1-2 commits and many changed lines, usually create semantic layers instead of commit layers.",
        "- Dependency order matters. If one atom uses code introduced by another, the provider/foundation atom must be in the same or a lower layer.",
        "- Imports are supporting noise. Attach import atoms to the substantive symbol/file change that requires them.",
        "- Tests usually belong with the behavior they validate. Use a separate test layer only for integration tests, test infrastructure, pre-refactor characterization tests, or broad acceptance coverage.",
        "- Refactors should be separate from behavior changes when they would otherwise obscure the behavior change.",
        "- The final layer must not become a garbage bucket. If the last layer contains more than 40% of substantive atoms or more than two unrelated concerns, split it.",
        "- Use manual_review_atom_ids for generated, binary, huge, ambiguous, or low-confidence atoms.",
        "- Preserve reviewer trust by making uncertainty explicit.",
        "- Do not hide intention gaps. If a layer's purpose cannot be reconstructed from PR text, atoms, semantic evidence, structural evidence, tests, commits, or discussions, lower confidence and add a warning.",
        "- Use review_question to ask the deepest useful reading question for that layer, not a generic validation task.",
        "- Favor layers that help a reviewer compare the implementation with their own expected design, especially for generated or mechanically large changes.",
        "- Prefer fewer coherent layers over many artificial layers.",
        "- Layer titles are compact UI labels: one line, 4-8 words preferred, 56 characters maximum, no full sentence, no code snippet, and no comma-separated symbol/key list.",
        "- Layer titles must be distinct; do not reuse the same title for multiple layers.",
        "- Put explanation, scope, verification detail, and uncertainty in review_question, summary, rationale, or warnings rather than title.",
        "",
        "Choose the dominant decomposition pattern:",
        "- dependency_chain: foundation/types/schema -> core logic -> integration/API -> UI -> broad tests/docs",
        "- refactor_then_change",
        "- mechanical_then_use for generated code, version bumps, schema regeneration, large formatting, or automated migrations",
        "- vertical_feature_slices when independent subfeatures are each reviewable end-to-end",
        "- risk_isolation",
        "- reviewer_boundary",
        "- comprehension_first: context/foundation -> behavior -> impact/callsites -> edge cases/tests -> historical or unresolved questions",
        "",
        "Substantive atoms include type/model/schema/API contracts, core behavior, algorithms, data/control flow, structural refactors, test behavior, integration/wiring, UI behavior, runtime config, generated code when it is the point of the layer, and version bumps.",
        "Non-substantive atoms include imports, formatting, comment-only edits, small rename fallout, mechanical call-site noise, and file reordering. Attach non-substantive atoms to the substantive atom that caused them.",
        "",
        "Before finalizing, run these checks:",
        "- no import-only layer",
        "- no misc/remaining/everything-else layer",
        "- no tail dump",
        "- every substantive atom assigned exactly once",
        "- every layer has one clear review question",
        "- dependency order is valid",
        "- generic tests-only layers are avoided unless they are integration, infrastructure, characterization, or broad acceptance coverage",
        "",
        "Return strict JSON only. No markdown, no prose outside JSON.",
        "",
        "Input:",
        &context_pretty,
        "",
        "Required output schema:",
        r#"{
  "strategy": "dependency_chain | refactor_then_change | mechanical_then_use | vertical_feature_slices | risk_isolation | reviewer_boundary | comprehension_first | semantic_virtual_stack | hybrid_virtual_stack | commit_virtual_stack | flat_manual_review",
  "confidence": "high | medium | low",
  "rationale": "short explanation",
  "layers": [
    {
      "title": "one-line review label, <=56 chars, 4-8 words, no code or comma list",
      "review_question": "what the reviewer should verify",
      "summary": "what this layer contains",
      "rationale": "why these atoms belong together and why this layer appears here",
      "substantive_atom_ids": ["existing substantive atom IDs only"],
      "attached_noise_atom_ids": ["existing import/formatting/comment atom IDs attached to the substantive change"],
      "depends_on_layer_indexes": [0],
      "confidence": "high | medium | low",
      "review_priority": "start_here | normal | quick_pass | manual_review"
    }
  ],
  "manual_review_atom_ids": ["existing atom IDs only"],
  "warnings": ["short warning strings"]
}"#,
    ]
    .join("\n")
}

pub fn build_stack_title_polish_prompt(input_json: &Value) -> String {
    let context_pretty = serde_json::to_string_pretty(input_json).expect("context must serialize");
    [
        "You are helping Remiss, a read-only pull request review IDE, polish deterministic review stack titles.",
        "",
        "The stack structure is already final. Your only task is to replace generic layer titles with compact, concrete UI labels.",
        "",
        "Hard rules:",
        "- Do not add, remove, reorder, merge, or split layers.",
        "- Do not change layer IDs, atom IDs, dependencies, summaries, or rationale.",
        "- Return exactly one title for each input layer ID.",
        "- If a deterministic title is already specific, keep it or make only a small improvement.",
        "- Use concrete nouns from the PR title, files, symbols, atom summaries, and layer role.",
        "- Avoid internal tool names, generic labels, comma-separated symbol lists, code snippets, and full sentences.",
        "- Titles must be one line, 3-8 words preferred, 56 characters maximum.",
        "",
        "Return strict JSON only. No markdown, no prose outside JSON.",
        "",
        "Input:",
        &context_pretty,
        "",
        "Required output schema:",
        r#"{
  "titles": [
    {
      "layerId": "existing input layer id",
      "title": "compact concrete layer label"
    }
  ]
}"#,
    ]
    .join("\n")
}

/// Build a follow-up prompt that asks the model to refine an earlier stack plan
/// after the response was produced but failed parsing or post-validation.
///
/// `failure_kind` should be a short label like "Parse error" or "Validation error".
/// `failure_message` is the specific failure reason from the parser/validator.
/// `previous_response` is the model's last raw response (typically JSON).
pub fn build_stack_planning_refinement_prompt(
    input_json: &Value,
    previous_response: &str,
    failure_kind: &str,
    failure_message: &str,
    attempt_number: usize,
    max_attempts: usize,
) -> String {
    const MAX_PREVIOUS_RESPONSE_CHARS: usize = 32_000;
    let trimmed_previous = trim_text(previous_response, MAX_PREVIOUS_RESPONSE_CHARS);
    let base = build_stack_planning_prompt(input_json);
    [
        base.as_str(),
        "",
        "Refinement instructions:",
        &format!(
            "This is attempt {} of {}. Your previous response was rejected by post-validation.",
            attempt_number, max_attempts
        ),
        &format!("{}: {}", failure_kind, failure_message),
        "",
        "Your previous response was:",
        &trimmed_previous,
        "",
        "Produce a corrected JSON plan that fixes only the specific problem above. Keep the rest of the plan intact when it was already correct.",
        "Do not over-correct: keep coherent multi-atom layers. Prefer fewer coherent layers over many single-atom layers. If atoms belong together by feature or dependency, keep them together even after the fix.",
        "If the failure is a tail-dump-style validation issue, prefer moving the offending atoms into the earlier layer whose behavior they support, rather than splitting them into many tiny layers.",
        "Return strict JSON only. No markdown, no prose outside JSON.",
    ]
    .join("\n")
}

pub fn trim_text(value: &str, max_length: usize) -> String {
    let normalized = value.trim();
    if normalized.chars().count() <= max_length {
        return normalized.to_string();
    }

    let truncated = normalized
        .chars()
        .take(max_length.saturating_sub(1))
        .collect::<String>();
    format!("{}…", truncated.trim_end())
}

pub fn linked_issue_refs<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut refs = Vec::<String>::new();
    let mut seen = std::collections::BTreeSet::<String>::new();

    for text in texts {
        for token in text.split_whitespace() {
            let trimmed = token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '.' | ';' | ':' | ')' | '(' | '[' | ']' | '{' | '}' | '"' | '\'' | '`'
                )
            });
            for issue_ref in issue_refs_from_token(trimmed) {
                if seen.insert(issue_ref.clone()) {
                    refs.push(issue_ref);
                    if refs.len() >= 12 {
                        return refs;
                    }
                }
            }
        }
    }

    refs
}

fn issue_refs_from_token(token: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let token = token.trim();
    if token.is_empty() {
        return refs;
    }

    if let Some(issue) = token.strip_prefix("https://github.com/") {
        let parts = issue.split('/').collect::<Vec<_>>();
        if parts.len() >= 4 && matches!(parts[2], "issues" | "pull") {
            let number = leading_digits(parts[3]);
            if !number.is_empty() {
                refs.push(format!("{}/{}#{}", parts[0], parts[1], number));
            }
        }
    }

    for (index, _) in token.match_indices('#') {
        let prefix = &token[..index];
        let rest = &token[index + 1..];
        let number = leading_digits(rest);
        if number.is_empty() {
            continue;
        }
        let repo_prefix = prefix
            .rsplit_once(char::is_whitespace)
            .map(|(_, suffix)| suffix)
            .unwrap_or(prefix)
            .trim_matches(|ch: char| matches!(ch, '(' | '[' | '{' | '"' | '\''));
        if repo_prefix.contains('/') && !repo_prefix.ends_with('/') {
            refs.push(format!("{repo_prefix}#{number}"));
        } else {
            refs.push(format!("#{number}"));
        }
    }

    refs
}

fn leading_digits(value: &str) -> &str {
    let end = value
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_ascii_digit()).then_some(index))
        .unwrap_or(value.len());
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stack_planning_prompt_uses_understanding_units() {
        let prompt = build_stack_planning_prompt(&json!({ "atoms": [] }));

        assert!(prompt.contains("understanding unit"));
        assert!(prompt.contains("deepest useful reading question"));
        assert!(prompt.contains("comprehension_first"));
        assert!(prompt.contains("Do not hide intention gaps"));
    }

    #[test]
    fn stack_title_polish_prompt_only_allows_titles() {
        let prompt = build_stack_title_polish_prompt(&json!({
            "layers": [{"id": "layer-1", "currentTitle": "Update src"}]
        }));

        assert!(prompt.contains("Your only task"));
        assert!(prompt.contains("Do not add, remove, reorder, merge, or split layers"));
        assert!(prompt.contains("\"layerId\""));
        assert!(!prompt.contains("substantive_atom_ids"));
    }

    #[test]
    fn trim_text_respects_character_limit() {
        let long = "あいうえお".repeat(50);
        let trimmed = trim_text(&long, 10);
        assert!(trimmed.chars().count() <= 10);
        assert!(trimmed.ends_with('…'));
    }
}
