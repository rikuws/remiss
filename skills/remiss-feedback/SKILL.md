---
name: remiss-feedback
description: Read and apply Remiss Local Changes feedback from app-data JSON files. Use when a user asks an agent to fetch Remiss feedback, apply Remiss local comments, or address Local Changes feedback for the current git checkout.
---

# Remiss Feedback

Use this skill when the user asks you to apply feedback captured in Remiss Local Changes. Remiss stores feedback outside the repository, under the user's app data directory, and the files are read-only from the agent's perspective.

## Locate Feedback

1. Resolve the current repository root:

```sh
git rev-parse --show-toplevel
```

2. Locate Remiss app data:

- macOS: `~/Library/Application Support/remiss/local-feedback`
- fallback for older installs: `~/Library/Application Support/gh-ui-tool/local-feedback`

3. Read `index.json` and find the entry whose `repoRoot` exactly matches the git root. Do not match by repository name alone.

4. Read that entry's `currentPath`. The document schema is `remiss.feedback.v1`.

## Validate Snapshot

Before editing files, verify the feedback still applies:

- `repoRoot` equals the current git root.
- `headRef` matches the current branch name.
- `headOid` matches `git rev-parse HEAD`.
- If `worktreeIdentity` is present, the Remiss document represents dirty working-tree state. If the current working tree has changed since Remiss exported the document, stop and ask the user to refresh Local Changes in Remiss.
- If `baseOid` is present, it must remain a valid commit in the local repository.

If validation fails, do not apply the comments. Tell the user the Remiss feedback is stale and needs a Local Changes refresh.

## Apply Feedback

- Use only comments with `status: "pending"`.
- Treat resolved or stale comments as history, not work to perform.
- Apply the minimal code changes needed to satisfy the feedback.
- Inspect surrounding code before editing; `selectedText`, `beforeContext`, `afterContext`, and line numbers are anchors, not a substitute for reading the file.
- Preserve unrelated local changes.
- Run relevant focused tests or checks when practical.
- Summarize changed files, addressed comments, and tests run.

## Do Not Write Back

Do not edit Remiss feedback files. V1 has no agent writeback contract for claim, applied, blocked, or replies. Report progress in your normal final response instead.
