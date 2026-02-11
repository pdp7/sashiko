# Design: Benchmark Validation

## Objective
Validate the accuracy of `sashiko`'s code review findings against a ground truth dataset (`benchmark.json`).

## Input Data
- **Source**: `benchmark.json`
- **Format**: Array of objects containing:
  - `Commit`: The commit hash (used as Message-ID or identifier).
  - `problem_description`: A text description of the bug/issue.
  - `subsystem`: Kernel subsystem (optional context).

## Validation Logic

The validation process will be implemented in `src/bin/benchmark_review.rs`.

1.  **Load Benchmark Data**: Read `benchmark.json`.
2.  **Iterate Entries**: Process each entry independently. Note that a single commit may have multiple problem entries.
3.  **Fetch Findings**:
    - Query the `sashiko` database (`patches` table) using the `Commit` hash (mapping to `message_id` or `git_blob_hash` depending on how it was ingested).
    - Retrieve the associated `reviews` and their `findings`.
4.  **LLM Comparison**:
    - Use Gemini (model configured in `Settings.toml`) to compare the `problem_description` against the list of `findings`.
    - **Strictness**: The comparison must check if *one* of the findings *exactly* describes the problem.
    - **Prompt Strategy**:
        - Provide the "Ground Truth" problem.
        - Provide the list of "Tool Findings" (including problem description, severity, and explanation).
        - Ask specifically: "Does any finding exactly describe the ground truth problem?"
        - Classify as `DETECTED` (exact match), `PARTIALLY_DETECTED` (vague or slightly off), or `MISSED`.
5.  **Reporting**:
    - Output detailed results to `benchmark_results.json`.
    - Print a summary to stdout (Detected, Partial, Missed counts).

## Implementation Details

### Database Lookup
- The `benchmark.json` "Commit" field likely corresponds to the `message_id` in the `patches` table if the patches were ingested from a git source where the commit hash is used as the ID.
- We need to handle cases where the patch might be missing or not reviewed.

### Gemini Prompt
```text
I am benchmarking an automated code review tool.

The known issue (ground truth) is:
{problem_description}

The tool produced the following findings:
{findings_list}

Task:
Determine if any of the findings EXACTLY describes the known issue.
- The description must match the specific problem (e.g., "memory leak in function X", "double free", "missing lock").
- General warnings about code style or unrelated bugs do NOT count.
- If a finding describes the problem but with slight inaccuracy (e.g. wrong variable name but correct logic), it is PARTIALLY_DETECTED.
- If no finding matches the problem, it is MISSED.

Respond with EXACTLY one of: [DETECTED, PARTIALLY_DETECTED, MISSED].
Then provide a short one-sentence explanation referencing the specific finding that matches (if any).
```

### Output Format
```json
[
  {
    "commit": "...",
    "problem_description": "...",
    "found": true,
    "status": "DETECTED", // DETECTED, PARTIALLY_DETECTED, MISSED, NOT_FOUND_IN_DB, NOT_REVIEWED, SKIPPED
    "explanation": "Finding #2 explicitly mentions the memory leak in...",
    "findings_count": 3
  },
  ...
]
```

## Usage

Ensure your `Settings.toml` is configured with a valid `ai.model` and database path.

Run the benchmark validation tool:
```bash
cargo run --bin benchmark_review
```

The tool will:
1. Load `benchmark.json`.
2. Iterate through entries, skipping those without a problem description.
3. Query the database for the corresponding patch and latest review.
4. Use the configured LLM to evaluate if the review findings match the known issue.
5. Generate `benchmark_results.json` with detailed results.
6. Print a summary to the console.
