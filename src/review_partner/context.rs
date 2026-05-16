use super::*;

pub(super) fn collect_review_partner_context(
    detail: &PullRequestDetail,
    stack: &ReviewStack,
    checkout_root: &Path,
    semantic_review: Option<&RemissSemanticReviewSummary>,
    lsp_session_manager: Option<&LspSessionManager>,
) -> ReviewPartnerContextPack {
    if !checkout_root.exists() {
        return ReviewPartnerContextPack {
            version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
            layers: Vec::new(),
            warnings: vec![format!(
                "Review Partner context could not inspect '{}'.",
                checkout_root.display()
            )],
        };
    }

    let atoms_by_id = stack
        .atoms
        .iter()
        .map(|atom| (atom.id.clone(), atom))
        .collect::<BTreeMap<_, _>>();
    let mut warnings = Vec::new();
    if let Some(semantic_review) = semantic_review {
        warnings.extend(
            semantic_review
                .warnings
                .iter()
                .take(MAX_SECTION_ITEMS)
                .map(|warning| format!("Semantic evidence: {warning}")),
        );
    }
    let layers = stack
        .layers
        .iter()
        .take(MAX_PARTNER_LAYERS)
        .map(|layer| {
            let atoms = layer
                .atom_ids
                .iter()
                .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
                .take(MAX_LAYER_ATOMS)
                .collect::<Vec<_>>();
            collect_layer_context(
                detail,
                layer,
                &atoms,
                checkout_root,
                semantic_review,
                lsp_session_manager,
                &mut warnings,
            )
        })
        .collect::<Vec<_>>();

    ReviewPartnerContextPack {
        version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
        layers,
        warnings,
    }
}

pub(super) fn collect_layer_context(
    detail: &PullRequestDetail,
    layer: &ReviewStackLayer,
    atoms: &[&ChangeAtom],
    checkout_root: &Path,
    semantic_review: Option<&RemissSemanticReviewSummary>,
    lsp_session_manager: Option<&LspSessionManager>,
    warnings: &mut Vec<String>,
) -> ReviewPartnerCollectedLayer {
    let semantic_layers = semantic_layers_for_layer(layer, semantic_review);
    let semantic_focus = semantic_focus_for_layer(layer, semantic_review);
    let changed_symbols =
        collect_changed_symbols(layer, atoms, checkout_root, lsp_session_manager, warnings);
    let removed_symbols = collect_removed_symbols(detail, atoms, checkout_root, warnings);
    let similar_locations = collect_similar_locations(
        &changed_symbols,
        checkout_root,
        MAX_SIMILAR_LOCATIONS_PER_LAYER,
    );
    let style_notes = collect_style_notes(atoms, checkout_root);

    let mut limitations = Vec::new();
    for symbol in changed_symbols.iter().chain(removed_symbols.iter()) {
        if symbol.reference_count > symbol.references.len() {
            limitations.push(format!(
                "{} has {} matching locations; showing {} representative locations.",
                symbol.symbol,
                symbol.reference_count,
                symbol.references.len()
            ));
        } else if symbol.search_strategy.contains("tree-sitter") {
            limitations.push(format!(
                "{} occurrences came from a bounded tree-sitter syntax scan.",
                symbol.symbol
            ));
        } else if symbol.search_strategy.contains("rg") {
            limitations.push(format!(
                "{} references came from a bounded text search.",
                symbol.symbol
            ));
        }
    }
    limitations.sort();
    limitations.dedup();

    ReviewPartnerCollectedLayer {
        layer_id: layer.id.clone(),
        semantic_layers,
        semantic_focus,
        changed_symbols,
        removed_symbols,
        similar_locations,
        style_notes,
        limitations: limitations.into_iter().take(MAX_SECTION_ITEMS).collect(),
    }
}

fn semantic_layers_for_layer(
    layer: &ReviewStackLayer,
    semantic_review: Option<&RemissSemanticReviewSummary>,
) -> Vec<ReviewPartnerSemanticLayer> {
    let Some(semantic_review) = semantic_review else {
        return Vec::new();
    };
    let layer_atom_ids = layer.atom_ids.iter().cloned().collect::<BTreeSet<_>>();
    semantic_review
        .layers
        .iter()
        .filter(|semantic_layer| {
            semantic_layer
                .atom_ids
                .iter()
                .any(|atom_id| layer_atom_ids.contains(atom_id))
        })
        .take(MAX_SECTION_ITEMS)
        .map(review_partner_semantic_layer)
        .collect()
}

fn semantic_focus_for_layer(
    layer: &ReviewStackLayer,
    semantic_review: Option<&RemissSemanticReviewSummary>,
) -> Vec<RemissSemanticFocusSummary> {
    let Some(semantic_review) = semantic_review else {
        return Vec::new();
    };
    let layer_atom_ids = layer.atom_ids.iter().cloned().collect::<BTreeSet<_>>();
    semantic_review
        .focus_summaries
        .iter()
        .filter(|focus| layer_atom_ids.contains(&focus.atom_id))
        .take(MAX_SECTION_ITEMS)
        .cloned()
        .collect()
}

fn review_partner_semantic_layer(layer: &RemissSemanticLayerSummary) -> ReviewPartnerSemanticLayer {
    ReviewPartnerSemanticLayer {
        id: layer.id.clone(),
        title: limit_text(layer.title.clone(), MAX_ITEM_TEXT_CHARS),
        summary: limit_text(layer.summary.clone(), MAX_ITEM_TEXT_CHARS),
        rationale: limit_text(layer.rationale.clone(), MAX_ITEM_TEXT_CHARS),
        atom_ids: layer
            .atom_ids
            .iter()
            .take(MAX_LAYER_ATOMS)
            .cloned()
            .collect(),
        file_paths: layer
            .file_paths
            .iter()
            .take(MAX_SECTION_ITEMS)
            .cloned()
            .collect(),
        hunk_indices: layer.hunk_indices.clone(),
        entity_names: layer
            .entity_names
            .iter()
            .take(MAX_SECTION_ITEMS)
            .cloned()
            .collect(),
        change_count: layer.change_count,
    }
}

fn collect_changed_symbols(
    layer: &ReviewStackLayer,
    atoms: &[&ChangeAtom],
    checkout_root: &Path,
    lsp_session_manager: Option<&LspSessionManager>,
    warnings: &mut Vec<String>,
) -> Vec<ReviewPartnerCollectedSymbol> {
    let mut seen = BTreeSet::<String>::new();
    let mut symbols = Vec::new();

    for atom in atoms {
        let mut candidates = atom.defined_symbols.clone();
        if let Some(symbol) = &atom.symbol_name {
            candidates.push(symbol.clone());
        }

        for symbol in candidates {
            let symbol = clean_symbol(&symbol);
            if !is_searchable_symbol(&symbol) || !seen.insert(symbol.clone()) {
                continue;
            }

            let tree_sitter_references =
                search_tree_sitter_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS);
            let (locations, reference_count, strategy) = match tree_sitter_references {
                Some(result) if !result.locations.is_empty() => {
                    (result.locations, result.reference_count, result.strategy)
                }
                _ => match references_for_symbol(
                    checkout_root,
                    lsp_session_manager,
                    atom,
                    &symbol,
                    MAX_REFERENCES_PER_SYMBOL,
                ) {
                    Ok(result) if !result.locations.is_empty() => (
                        result.locations,
                        result.reference_count,
                        result.strategy.to_string(),
                    ),
                    _ => {
                        let result =
                            search_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS);
                        if let Some(error) = result.warning {
                            warnings.push(format!("{}: {error}", layer.title));
                        }
                        (result.locations, result.reference_count, result.strategy)
                    }
                },
            };

            symbols.push(ReviewPartnerCollectedSymbol {
                symbol,
                path: atom.path.clone(),
                line: atom.new_range.and_then(line_from_range),
                atom_ids: vec![atom.id.clone()],
                search_strategy: strategy,
                reference_count,
                references: locations
                    .into_iter()
                    .take(MAX_REFERENCES_PER_SYMBOL)
                    .collect(),
            });

            if symbols.len() >= MAX_CONTEXT_SYMBOLS_PER_LAYER {
                return symbols;
            }
        }
    }

    symbols
}

pub(super) fn collect_removed_symbols(
    detail: &PullRequestDetail,
    atoms: &[&ChangeAtom],
    checkout_root: &Path,
    warnings: &mut Vec<String>,
) -> Vec<ReviewPartnerCollectedSymbol> {
    let mut removed = Vec::new();
    let mut seen = BTreeSet::<String>::new();

    for atom in atoms {
        let Some(parsed) = find_parsed_diff_file(&detail.parsed_diff, &atom.path) else {
            continue;
        };
        for (symbol, line) in removed_declarations_for_atom(parsed, atom) {
            let key = format!("{}:{}:{}", atom.path, symbol, line.unwrap_or_default());
            if !seen.insert(key) {
                continue;
            }
            let result =
                search_tree_sitter_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS)
                    .unwrap_or_else(|| {
                        search_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS)
                    });
            if let Some(error) = result.warning {
                warnings.push(format!("{}: {error}", atom.path));
            }
            removed.push(ReviewPartnerCollectedSymbol {
                symbol,
                path: atom.path.clone(),
                line,
                atom_ids: vec![atom.id.clone()],
                search_strategy: result.strategy,
                reference_count: result.reference_count,
                references: result
                    .locations
                    .into_iter()
                    .take(MAX_REFERENCES_PER_SYMBOL)
                    .collect(),
            });
            if removed.len() >= MAX_CONTEXT_SYMBOLS_PER_LAYER {
                return removed;
            }
        }
    }

    removed
}

fn removed_declarations_for_atom(
    parsed: &ParsedDiffFile,
    atom: &ChangeAtom,
) -> Vec<(String, Option<usize>)> {
    let mut removed = Vec::new();
    let mut hunk_indices = atom.hunk_indices.iter().copied().collect::<BTreeSet<_>>();
    if hunk_indices.is_empty() {
        hunk_indices.extend(0..parsed.hunks.len());
    }

    for (index, hunk) in parsed.hunks.iter().enumerate() {
        if !hunk_indices.contains(&index) {
            continue;
        }
        for line in &hunk.lines {
            if line.kind != DiffLineKind::Deletion {
                continue;
            }
            if let Some(symbol) = declaration_symbol(&line.content) {
                removed.push((
                    symbol,
                    line.left_line_number
                        .and_then(|line| usize::try_from(line).ok()),
                ));
            }
        }
    }

    removed
}

struct SymbolReferenceResult {
    locations: Vec<ReviewPartnerLocation>,
    reference_count: usize,
    strategy: &'static str,
}

fn references_for_symbol(
    checkout_root: &Path,
    lsp_session_manager: Option<&LspSessionManager>,
    atom: &ChangeAtom,
    symbol: &str,
    limit: usize,
) -> Result<SymbolReferenceResult, String> {
    let Some(lsp_session_manager) = lsp_session_manager else {
        return Err("LSP unavailable".to_string());
    };
    let document_path = checkout_root.join(&atom.path);
    let document = fs::read_to_string(&document_path)
        .map_err(|error| format!("Failed to read {}: {error}", atom.path))?;
    let Some((line, column)) = symbol_position_in_document(&document, atom.new_range, symbol)
    else {
        return Err(format!("Could not locate {symbol} in {}", atom.path));
    };
    let document_text: Arc<str> = Arc::from(document.as_str());
    let request = LspTextDocumentRequest {
        file_path: atom.path.clone(),
        document_text,
        line,
        column,
    };
    let details = lsp_session_manager.symbol_details(checkout_root, &request)?;
    let reference_count = details.reference_targets.len();
    let locations = details
        .reference_targets
        .into_iter()
        .take(limit)
        .map(|target| {
            let snippet = read_checkout_line(checkout_root, &target.path, target.line);
            ReviewPartnerLocation {
                path: target.path,
                line: target.line,
                snippet,
            }
        })
        .collect::<Vec<_>>();

    Ok(SymbolReferenceResult {
        locations,
        reference_count,
        strategy: "lsp references",
    })
}

struct SearchResult {
    locations: Vec<ReviewPartnerLocation>,
    reference_count: usize,
    strategy: String,
    warning: Option<String>,
}

#[derive(Clone, Copy)]
enum SearchMode {
    Identifier,
    Text,
}

fn search_tree_sitter_symbol_locations(
    checkout_root: &Path,
    symbol: &str,
    limit: usize,
) -> Option<SearchResult> {
    if !is_searchable_symbol(symbol) {
        return None;
    }

    let mut parser = Parser::new();
    let language = tree_sitter_rust_orchard::LANGUAGE.into();
    parser.set_language(&language).ok()?;

    let mut queue = VecDeque::from([(checkout_root.to_path_buf(), 0usize)]);
    let mut scanned_files = 0usize;
    let mut locations = Vec::new();
    let mut seen = BTreeSet::<String>::new();
    let mut reference_count = 0usize;

    while let Some((path, depth)) = queue.pop_front() {
        if depth > MAX_SCAN_DEPTH || scanned_files >= MAX_SCAN_FILES {
            break;
        }
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                if should_skip_directory(file_name) {
                    continue;
                }
                queue.push_back((path, depth + 1));
                continue;
            }
            if scanned_files >= MAX_SCAN_FILES || !is_tree_sitter_rust_candidate(&path) {
                continue;
            }
            scanned_files += 1;
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_SCAN_FILE_BYTES {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Some(tree) = parser.parse(&text, None) else {
                continue;
            };
            let lines = text.lines().collect::<Vec<_>>();
            collect_tree_sitter_symbol_locations(
                tree.root_node(),
                &text,
                &lines,
                checkout_root,
                &path,
                symbol,
                limit,
                &mut locations,
                &mut seen,
                &mut reference_count,
            );
        }
    }

    Some(SearchResult {
        locations,
        reference_count,
        strategy: "tree-sitter rust identifier scan".to_string(),
        warning: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_tree_sitter_symbol_locations(
    node: Node<'_>,
    text: &str,
    lines: &[&str],
    checkout_root: &Path,
    path: &Path,
    symbol: &str,
    limit: usize,
    locations: &mut Vec<ReviewPartnerLocation>,
    seen: &mut BTreeSet<String>,
    reference_count: &mut usize,
) {
    if tree_sitter_node_matches_symbol(node, text, symbol) {
        let line = node.start_position().row + 1;
        let relative = relative_path(checkout_root, path);
        let key = format!("{relative}:{line}");
        if seen.insert(key) {
            *reference_count += 1;
            if locations.len() < limit {
                locations.push(ReviewPartnerLocation {
                    path: relative,
                    line,
                    snippet: lines
                        .get(line.saturating_sub(1))
                        .map(|line| trim_text(line, MAX_PROMPT_SNIPPET_CHARS)),
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tree_sitter_symbol_locations(
            child,
            text,
            lines,
            checkout_root,
            path,
            symbol,
            limit,
            locations,
            seen,
            reference_count,
        );
    }
}

fn tree_sitter_node_matches_symbol(node: Node<'_>, text: &str, symbol: &str) -> bool {
    let kind = node.kind();
    if kind != "identifier" && !kind.ends_with("_identifier") {
        return false;
    }
    node.utf8_text(text.as_bytes())
        .map(|value| value == symbol)
        .unwrap_or(false)
}

fn is_tree_sitter_rust_candidate(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension == "rs")
        .unwrap_or(false)
}

fn search_symbol_locations(checkout_root: &Path, symbol: &str, limit: usize) -> SearchResult {
    search_locations_in_scope(checkout_root, symbol, limit, None, SearchMode::Identifier)
}

fn search_similar_locations_in_scope(
    checkout_root: &Path,
    token: &str,
    limit: usize,
    relative_scope: Option<&Path>,
) -> SearchResult {
    search_locations_in_scope(
        checkout_root,
        token,
        limit,
        relative_scope,
        SearchMode::Text,
    )
}

fn search_locations_in_scope(
    checkout_root: &Path,
    symbol: &str,
    limit: usize,
    relative_scope: Option<&Path>,
    mode: SearchMode,
) -> SearchResult {
    match rg_symbol_locations(checkout_root, symbol, limit, relative_scope, mode) {
        Ok(result) => result,
        Err(error) => {
            let start = relative_scope
                .filter(|scope| !scope.as_os_str().is_empty())
                .map(|scope| checkout_root.join(scope))
                .unwrap_or_else(|| checkout_root.to_path_buf());
            let mut result = scan_symbol_locations_from(checkout_root, &start, symbol, limit, mode);
            result.warning = Some(format!("rg search unavailable, used bounded scan: {error}"));
            result
        }
    }
}

fn rg_symbol_locations(
    checkout_root: &Path,
    symbol: &str,
    limit: usize,
    relative_scope: Option<&Path>,
    mode: SearchMode,
) -> Result<SearchResult, String> {
    let search_path = relative_scope
        .filter(|scope| !scope.as_os_str().is_empty())
        .and_then(|scope| scope.to_str())
        .unwrap_or(".");
    let mut command = Command::new("rg");
    command
        .arg("--line-number")
        .arg("--fixed-strings")
        .arg("--color")
        .arg("never")
        .arg("--max-count")
        .arg("5")
        .arg("--glob")
        .arg("!.git");
    if matches!(mode, SearchMode::Text) {
        command.arg("--ignore-case");
    }
    let output = command
        .arg("--")
        .arg(symbol)
        .arg(search_path)
        .current_dir(checkout_root)
        .output()
        .map_err(|error| error.to_string())?;

    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut locations = Vec::new();
    let mut reference_count = 0usize;
    for line in stdout.lines() {
        let Some(location) = parse_rg_line(line) else {
            continue;
        };
        if !location_matches_search(&location, symbol, mode) {
            continue;
        }
        reference_count += 1;
        if locations.len() < limit {
            locations.push(location);
        }
    }

    Ok(SearchResult {
        locations,
        reference_count,
        strategy: "rg exact text search".to_string(),
        warning: None,
    })
}

fn scan_symbol_locations(checkout_root: &Path, symbol: &str, limit: usize) -> SearchResult {
    scan_symbol_locations_from(
        checkout_root,
        checkout_root,
        symbol,
        limit,
        SearchMode::Identifier,
    )
}

fn scan_symbol_locations_from(
    checkout_root: &Path,
    start_path: &Path,
    symbol: &str,
    limit: usize,
    mode: SearchMode,
) -> SearchResult {
    let mut queue = VecDeque::from([(start_path.to_path_buf(), 0usize)]);
    let mut scanned_files = 0usize;
    let mut locations = Vec::new();
    let mut reference_count = 0usize;

    while let Some((path, depth)) = queue.pop_front() {
        if depth > MAX_SCAN_DEPTH || scanned_files >= MAX_SCAN_FILES {
            break;
        }
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                if should_skip_directory(file_name) {
                    continue;
                }
                queue.push_back((path, depth + 1));
                continue;
            }
            if scanned_files >= MAX_SCAN_FILES || !is_text_search_candidate(&path) {
                continue;
            }
            scanned_files += 1;
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_SCAN_FILE_BYTES {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if is_comment_only_line(line) || !line_matches_search(line, symbol, mode) {
                    continue;
                }
                reference_count += 1;
                if locations.len() < limit {
                    locations.push(ReviewPartnerLocation {
                        path: relative_path(checkout_root, &path),
                        line: index + 1,
                        snippet: Some(trim_text(line, MAX_PROMPT_SNIPPET_CHARS)),
                    });
                }
            }
        }
    }

    SearchResult {
        locations,
        reference_count,
        strategy: "bounded file scan".to_string(),
        warning: None,
    }
}

fn parse_rg_line(line: &str) -> Option<ReviewPartnerLocation> {
    let mut parts = line.splitn(3, ':');
    let path = parts.next()?.trim_start_matches("./").to_string();
    let line_number = parts.next()?.parse::<usize>().ok()?;
    let snippet = parts
        .next()
        .map(|value| trim_text(value, MAX_PROMPT_SNIPPET_CHARS));
    Some(ReviewPartnerLocation {
        path,
        line: line_number,
        snippet,
    })
}

fn location_matches_search(
    location: &ReviewPartnerLocation,
    symbol: &str,
    mode: SearchMode,
) -> bool {
    location
        .snippet
        .as_deref()
        .map(|snippet| !is_comment_only_line(snippet) && line_matches_search(snippet, symbol, mode))
        .unwrap_or(false)
}

fn is_comment_only_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with("--")
        || trimmed.starts_with("<!--")
        || trimmed.starts_with("# ")
}

fn line_matches_search(line: &str, symbol: &str, mode: SearchMode) -> bool {
    match mode {
        SearchMode::Identifier => contains_identifier(line, symbol),
        SearchMode::Text => line
            .to_ascii_lowercase()
            .contains(&symbol.to_ascii_lowercase()),
    }
}

pub(super) fn collect_similar_locations(
    symbols: &[ReviewPartnerCollectedSymbol],
    checkout_root: &Path,
    limit: usize,
) -> Vec<ReviewPartnerLocation> {
    let mut locations = Vec::new();
    let mut seen = BTreeSet::<String>::new();
    for symbol in symbols {
        let Some(token) = similar_search_token(&symbol.symbol) else {
            continue;
        };

        let module_scope = Path::new(&symbol.path)
            .parent()
            .filter(|scope| !scope.as_os_str().is_empty());
        let mut scoped_results = Vec::new();
        if let Some(scope) = module_scope {
            scoped_results.push(search_similar_locations_in_scope(
                checkout_root,
                &token,
                limit,
                Some(scope),
            ));
        }
        scoped_results.push(search_similar_locations_in_scope(
            checkout_root,
            &token,
            limit,
            None,
        ));

        for result in scoped_results {
            for location in result.locations {
                if location.path == symbol.path && Some(location.line) == symbol.line {
                    continue;
                }
                let key = format!("{}:{}", location.path, location.line);
                if seen.insert(key) {
                    locations.push(location);
                    if locations.len() >= limit {
                        return locations;
                    }
                }
            }
        }
    }
    locations
}

fn collect_style_notes(atoms: &[&ChangeAtom], checkout_root: &Path) -> Vec<ReviewPartnerItem> {
    let mut notes = Vec::new();
    let mut seen = BTreeSet::<String>::new();
    for atom in atoms {
        if let Some(note) = nearby_style_note(atom, checkout_root) {
            if seen.insert(note.title.clone()) {
                notes.push(note);
                if notes.len() >= MAX_STYLE_NOTES_PER_LAYER {
                    break;
                }
            }
        }
    }
    notes
}

fn nearby_style_note(atom: &ChangeAtom, checkout_root: &Path) -> Option<ReviewPartnerItem> {
    let path = Path::new(&atom.path);
    let directory = path.parent()?;
    let checkout_directory = checkout_root.join(directory);
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let entries = fs::read_dir(&checkout_directory).ok()?;
    let mut siblings = entries
        .flatten()
        .filter_map(|entry| {
            let candidate = entry.path();
            if candidate.is_dir() || candidate == checkout_root.join(&atom.path) {
                return None;
            }
            let same_extension = candidate
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext == extension)
                .unwrap_or(false);
            same_extension.then(|| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string()
            })
        })
        .take(5)
        .collect::<Vec<_>>();
    siblings.sort();
    if siblings.is_empty() {
        return None;
    }
    Some(ReviewPartnerItem::new(
        format!("Nearby {}", directory.display()),
        format!(
            "Sibling files for style comparison: {}.",
            siblings.join(", ")
        ),
        Some(atom.path.clone()),
        atom.new_range.and_then(line_from_range),
    ))
}

pub(super) fn items_from_changed_symbols(
    context: &ReviewPartnerCollectedLayer,
) -> Vec<ReviewPartnerItem> {
    context
        .changed_symbols
        .iter()
        .map(|symbol| {
            ReviewPartnerItem::new(
                symbol.symbol.clone(),
                format!(
                    "Changed in {}{}; {} reference{} surfaced via {}.",
                    symbol.path,
                    symbol
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default(),
                    symbol.reference_count,
                    if symbol.reference_count == 1 { "" } else { "s" },
                    symbol.search_strategy,
                ),
                Some(symbol.path.clone()),
                symbol.line,
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

pub(super) fn items_from_semantic_layers(
    context: &ReviewPartnerCollectedLayer,
) -> Vec<ReviewPartnerItem> {
    context
        .semantic_layers
        .iter()
        .map(|layer| {
            ReviewPartnerItem::new(
                layer.title.clone(),
                default_if_empty(layer.summary.clone(), &layer.rationale),
                layer.file_paths.first().cloned(),
                None,
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

pub(super) fn items_from_semantic_focus(
    context: &ReviewPartnerCollectedLayer,
) -> Vec<ReviewPartnerItem> {
    context
        .semantic_focus
        .iter()
        .filter_map(|focus| {
            let entity = focus
                .target_entity
                .as_ref()
                .or_else(|| focus.overlapping_entities.first())?;
            let impact_detail = focus.impact.as_ref().map(|impact| {
                format!(
                    " Sem found {} dependenc{}, {} dependent{}, and {} test target{}.",
                    impact.dependencies.len(),
                    if impact.dependencies.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    },
                    impact.dependents.len(),
                    if impact.dependents.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    impact.tests.len(),
                    if impact.tests.len() == 1 { "" } else { "s" }
                )
            });
            Some(ReviewPartnerItem::new(
                entity.name.clone(),
                format!(
                    "Sem resolved this layer through the {} `{}` in {}:{}-{}.{}",
                    entity.entity_type,
                    entity.name,
                    entity.file_path,
                    entity.start_line,
                    entity.end_line,
                    impact_detail.unwrap_or_default()
                ),
                Some(entity.file_path.clone()),
                Some(entity.start_line),
            ))
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

pub(super) fn items_from_removed_symbols(
    context: &ReviewPartnerCollectedLayer,
) -> Vec<ReviewPartnerItem> {
    context
        .removed_symbols
        .iter()
        .map(|symbol| {
            let detail = if symbol.reference_count == 0 {
                format!(
                    "Removed from {}{}; no remaining references surfaced in the bounded scan.",
                    symbol.path,
                    symbol
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default()
                )
            } else {
                format!(
                    "Removed from {}{}; {} remaining match{} surfaced.",
                    symbol.path,
                    symbol
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default(),
                    symbol.reference_count,
                    if symbol.reference_count == 1 {
                        ""
                    } else {
                        "es"
                    },
                )
            };
            ReviewPartnerItem::new(
                symbol.symbol.clone(),
                detail,
                Some(symbol.path.clone()),
                symbol.line,
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

pub(super) fn items_from_usages(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .changed_symbols
        .iter()
        .chain(context.removed_symbols.iter())
        .flat_map(|symbol| {
            symbol.references.iter().map(move |location| {
                ReviewPartnerItem::new(
                    symbol.symbol.clone(),
                    location
                        .snippet
                        .clone()
                        .unwrap_or_else(|| format!("Reference in {}", location.path)),
                    Some(location.path.clone()),
                    Some(location.line),
                )
            })
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

pub(super) fn items_from_similar_locations(
    context: &ReviewPartnerCollectedLayer,
) -> Vec<ReviewPartnerItem> {
    context
        .similar_locations
        .iter()
        .map(|location| {
            ReviewPartnerItem::new(
                format!("{}:{}", location.path, location.line),
                location
                    .snippet
                    .clone()
                    .unwrap_or_else(|| "Similar symbol context.".to_string()),
                Some(location.path.clone()),
                Some(location.line),
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

pub(super) fn items_from_style_notes(
    context: &ReviewPartnerCollectedLayer,
) -> Vec<ReviewPartnerItem> {
    context
        .style_notes
        .iter()
        .take(MAX_SECTION_ITEMS)
        .cloned()
        .collect()
}

pub(super) fn items_from_layer_atoms(
    layer: &ReviewStackLayer,
    stack: &ReviewStack,
) -> Vec<ReviewPartnerItem> {
    stack
        .atoms_for_layer(layer)
        .into_iter()
        .map(|atom| {
            ReviewPartnerItem::new(
                atom.symbol_name
                    .clone()
                    .unwrap_or_else(|| atom.path.clone()),
                format!(
                    "{} changed line{} in {}.",
                    atom.additions + atom.deletions,
                    if atom.additions + atom.deletions == 1 {
                        ""
                    } else {
                        "s"
                    },
                    atom.path
                ),
                Some(atom.path.clone()),
                atom.new_range.and_then(line_from_range),
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}
