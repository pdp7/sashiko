# Sashiko Implementation TODO

## Phase 1: Foundation
- [x] Initialize Rust workspace and project structure.
- [x] Set up `config` crate for environment/file-based configuration.
- [x] Implement NNTP Ingestor (polling `nntp.lore.kernel.org`).
- [x] Set up libSQL/Turso schema for mailing lists and patchsets.
- [x] Implement internal task queue (tokio channels).
- [x] Implement article state tracking (high-water mark).
- [x] Set up structured logging (`tracing`) and observability baseline.

## Phase 2: Git Ops & Patch Processing
- [x] Implement Patch/Patchset parsing from raw emails.
- [x] Develop baseline detection logic (explicit and heuristic).
- [x] Set up sandboxed `git am` environment.
- [x] Implement Worktree Garbage Collector (pruning & disk limits).
- [x] Refactor data model to Messages/Threads/Patches/Patchsets.
- [x] **Task**: Download `lore.kernel.org` git archive (e.g., 2025 emails) for offline testing.
- [x] **Task**: Implement file-based/git-based ingestor to process downloaded archives.
- [x] **Task**: Add configuration option to disable NNTP and use local archives.
- [x] **Task**: Implement automatic git archive bootstrapping with `--n-last`.
- [x] Implement patchset assembly (handling multi-part messages).
- [x] Implement Tagging System (Messages, Threads, Patches, Patchsets).
- [ ] Support parsing patches from email attachments.

## Phase 2.5: Performance Optimization
- [x] **Task**: Implement Transactional Batching (group DB writes to reduce I/O).
- [x] **Task**: Implement Decoupled Parallel Parsing (separate parsing from DB writes).
- [x] **Task**: Implement Git-Backed Content Storage (store hashes, read from git, reduce DB size).

## Phase 3: AI Logic & Interaction Tracking (In Progress)
- [x] Implement model-agnostic AI provider abstraction.
- [ ] Set up `ai_interactions` table and workflow engine for chain restoration.
- [x] Integrate with `review-prompts` repository logic.
- [ ] Implement consensus/comparison logic for multiple LLM runs.
- [x] **Task**: Implement `sashiko-review` agent with Gemini 3 Pro and Git Tools.
- [x] **Task**: Integrate reviewer tool into main Sashiko loop (concurrent processing).
- [x] **Task**: Implement Chain-of-Thought prompting and structured JSON output for reviews.
- [x] **Task**: Implement retry logic for AI review generation failures when patches apply successfully.
- [x] **Task**: Implement `review-inline.txt` support for detailed inline comments.
- [x] **Task**: Implement Context Budgeting and Content Truncation (TokenBudget, Smart Pruning).
- [ ] **Task**: Implement "Critic" self-correction loop for review validation.

## Phase 4: Web API & Frontend
- [x] Build Axum REST API for patches, reviews, and stats.
- [ ] Implement RBAC (Role-Based Access Control) for API endpoints (Deferred).
- [x] Implement minimalistic "kernel.org style" frontend (Raw HTML/JS/CSS).
- [x] Implement individual message view (metadata + body + copy ID).
- [x] Implement detailed AI interaction log view and API endpoint.
- [x] Polish frontend diff rendering and code block styling.
- [ ] Add UI for re-running reviews and manual baseline overrides.
- [ ] Configure Nginx as static file server and reverse proxy.

## Phase 5: Email Loop & Refinement
- [ ] Implement outbound SMTP for review replies.
- [ ] Implement inbound email processing (reply tracking).
- [ ] Add in-memory LRU cache for recent articles.
- [x] Stress test system with 20k/day message volume simulation (Verified with 100k messages).
- [ ] Final performance tuning and security hardening.