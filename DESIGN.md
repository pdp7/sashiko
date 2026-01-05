# Sashiko Design Document

Sashiko is a distributed system designed to track, apply, and review Linux Kernel changes automatically. It ingests patches from `lore.kernel.org` via NNTP, identifies their target baselines, applies them in a sandboxed environment, and uses Large Language Models (LLMs) to provide automated code reviews.

## 1. System Architecture

Sashiko follows a modular, single-node architecture designed for high concurrency using Rust's async runtime. It avoids external message brokers like Redis, relying instead on efficient in-memory channels and database polling for persistence.

### 1.1. High-Level Component Flow

1.  **Ingestor**:
    *   **Live Mode**: Continuously polls `nntp.lore.kernel.org` via NNTP.
    *   **Offline/Test Mode**: Reads from a local `git` clone of `lore.kernel.org` archives (bulk import).
2.  **Internal Task Queue**: Uses `tokio::sync::mpsc` channels to pass metadata to workers.
3.  **Patch Worker**:
    *   Parses emails into `Patch` and `Patchset` structures.
    *   Assembles multi-part patchsets.
    *   Detects the target git baseline.
    *   Applies patches in a sandboxed Git worktree.
4.  **AI Review Worker**: Sends applied patches to LLMs and processes feedback.
5.  **Database (Turso/libSQL)**: Stores all metadata, patches, and review results.
6.  **Web API (Axum)**: Serves data to the frontend.
7.  **Web Frontend**: Minimalistic, raw HTML and JavaScript interface served via Nginx.
8.  **Email Gateway**: Handles outbound reviews and inbound replies.

## 2. Component Details
...
### 2.5. Web Frontend
*   **Design Philosophy**: Simple and minimalistic, adhering to the "kernel.org vibes" (fast, text-heavy, no-nonsense).
*   **Technology Stack**: Standard HTML5, CSS, and Vanilla JavaScript. Served as static files by **Nginx**, which also acts as a reverse proxy for the Rust backend.
*   **Key Features**:
    *   **Patchset List**: The main view displays all tracked patchsets, searchable and filterable by status, author, or subsystem.
    *   **Interactive Review Management**: Button to re-trigger AI reviews for specific patchsets.
    *   **Manual Override**: Interface to manually specify or correct the `git` baseline (repo URL or commit hash) if automated detection fails.
    *   **Live Status**: Visual indication of the patchset's position in the processing pipeline.
*   **User Roles & Access Modes**:
    *   **Public / Individual Contributor**: Read-only access to patches and reviews. Can view their own submissions and system-wide review status.
    *   **Subsystem Maintainer**: Can trigger/re-run reviews, manually override git baselines for relevant subsystems, and manage patchset status (e.g., mark as "Ignore").
    *   **Admin**: Full access to system configuration, AI interaction logs, provider settings, and user management. Can force-clear queues or reset high-water marks.

## 3. Data Schema (libSQL)

The data model is built around five core entities: **Messages**, **Threads**, **Patches**, **Patchsets**, and **Subsystems**.

*   **Message**: Represents a single email received via NNTP.
*   **Thread**: A conversation consisting of multiple messages (linked via `In-Reply-To` and `References`).
*   **Patch**: A code change (diff) contained within a specific message.
*   **Patchset**: A logical collection of patches (e.g., a series 1/N, 2/N) intended to be applied together.
*   **Subsystem**: A logical grouping of changes based on Linux kernel subsystems (e.g., "netdev", "bpf", "usb"), determined by the mailing lists in To/Cc headers.

### 3.1. Tables

*   **`mailing_lists`**: `id`, `name`, `nntp_group`, `last_article_num`.
*   **`subsystems`**: `id`, `name`, `mailing_list_address` (e.g., `netdev@vger.kernel.org`).
*   **`threads`**:
    *   `id`: Primary Key.
    *   `root_message_id`: The Message-ID of the thread starter.
    *   `subject`: The subject of the root message.
    *   `last_updated`: Timestamp of the most recent message in the thread.
*   **`messages`**:
    *   `id`: Primary Key.
    *   `message_id`: String, Unique (from email headers).
    *   `thread_id`: Foreign Key to `threads`.
    *   `in_reply_to`: Nullable, Message-ID of the parent message.
    *   `author`: Sender's name/email.
    *   `subject`: Email subject.
    *   `date`: Timestamp.
    *   `body`: The full text content.
*   **`patchsets`**:
    *   `id`: Primary Key.
    *   `thread_id`: Foreign Key to `threads` (A patchset typically corresponds to a thread).
    *   `cover_letter_message_id`: Nullable, FK to `messages` (if a [PATCH 0/N] exists).
    *   `subject`: Title of the series.
    *   `status`: Pending, Assembled, Applied, Failed, Reviewed.
    *   `total_parts`: Expected number of patches.
    *   `received_parts`: Count of patches currently ingested.
    *   `baseline_id`: Foreign Key to `baselines`.
*   **`patches`**:
    *   `id`: Primary Key.
    *   `patchset_id`: Foreign Key to `patchsets`.
    *   `message_id`: Foreign Key to `messages` (The source message containing this patch).
    *   `part_index`: The index in the series (e.g., 1 for [PATCH 1/3]).
    *   `diff`: The extracted diff content.
*   **`baselines`**: `id`, `repo_url`, `branch`, `last_known_commit`.
*   **`reviews`**: `id`, `patchset_id`, `model_name`, `summary`, `created_at`.
*   **`comments`**: `id`, `review_id`, `file_path`, `line_number`, `content`, `severity` (Info, Warning, Error).
*   **`ai_interactions`**:
    *   `id` (UUID): Unique identifier for the interaction.
    *   `parent_interaction_id` (UUID, nullable): For chaining operations (e.g., refinement steps).
    *   `workflow_id` (UUID): To group all steps of a complex operation (e.g., "Review Patchset X").
    *   `provider`: Provider name (e.g., "openai", "anthropic").
    *   `model`: Specific model used (e.g., "gpt-4-turbo", "claude-3-opus").
    *   `input_context`: Full prompt/context sent to the LLM (stored as JSON or text).
    *   `output_raw`: Raw response received from the LLM.
    *   `tokens_in`, `tokens_out`: Usage metrics for cost tracking.
    *   `created_at`: Timestamp.
*   **Junction Tables**:
    *   `messages_subsystems`: `message_id` (FK), `subsystem_id` (FK).
    *   `threads_subsystems`: `thread_id` (FK), `subsystem_id` (FK).
    *   `patches_subsystems`: `patch_id` (FK), `subsystem_id` (FK).
    *   `patchsets_subsystems`: `patchset_id` (FK), `subsystem_id` (FK).

## 4. Scalability & Reliability

*   **Performance Targets**: Designed to process up to **20,000 emails per day** on a single instance.
*   **Asynchronous Processing**:
    *   AI operations are the primary bottleneck. They are decoupled from ingestion using bounded channels with backpressure.
    *   The `AI Review Worker` runs independently, processing tasks at the rate allowed by external API limits, without blocking the NNTP ingestor or Web API.
*   **Concurrency**: Rust's `tokio` runtime efficiently handles thousands of concurrent connections and tasks within a single process.
*   **Database**: libSQL (Turso) handles persistence.
*   **Storage**: Git clones are large. Workers use a shared `reference` repository to minimize disk usage (via `git clone --reference`).
*   **Fault Tolerance**:
    *   **Crash Recovery**: In-memory channels are volatile. To ensure zero data loss, all incoming metadata is **committed to libSQL immediately upon receipt** before being queued for processing.
    *   **State Reconstruction**: On startup, the system scans the database for items in `Pending` or `Applying` states and re-queues them.
    *   **Error Handling**: Transient AI errors trigger exponential backoff retries. Permanent failures are logged and the patchset status is updated to `Failed` (requiring manual intervention), preventing head-of-line blocking.

## 5. Technical Specifications

### 5.1. AI Prompting
Sashiko integrates with the AI Review Engine using prompts managed in the [review-prompts](https://github.com/masoncl/review-prompts) repository. It handles the injection of patch context (diffs, metadata) into these templates and manages the resulting interaction chain.

For a detailed breakdown of the prompting strategy, context management, and stability heuristics, refer to `DESIGN_LLM_REVIEW_STRATEGY.md`.

### 5.2. Web API Endpoints
*   `GET /api/patchsets`: List latest patchsets with status and filtering (e.g., `?status=Reviewed`).
*   `GET /api/patchsets/:id`: Detailed view of a patchset, including all patches and reviews.
*   `GET /api/reviews/:id`: Detailed AI review comments.
*   `POST /api/reviews/:id/re-run`: Trigger a manual re-review.
*   `POST /api/patchsets/:id/baseline`: Manually update the git baseline for a patchset.
*   `GET /api/stats`: System-wide statistics (patches processed, AI costs, etc.).

## 6. Security

*   **Access Control**: Role-Based Access Control (RBAC) enforced at the API level to distinguish between Public, Maintainer, and Admin actions.
*   **Sandboxing**: All `git` operations on untrusted patches are isolated.
*   **Rate Limiting**: AI API calls are rate-limited per author/patchset to prevent abuse and cost overruns.
*   **Data Integrity**: Use of content-addressable storage (hashes) for patch verification.

## 7. Observability and Logging

*   **Consistent Logging**: Every component must implement high-quality, structured logging (using the `tracing` crate). This is essential for production stability and rapid root-cause analysis.
*   **Contextual Metadata**: Logs must include relevant context such as `patchset_id`, `message_id`, or `workflow_id` to allow for easy correlation across different stages of processing.
*   **Monitoring**: Export metrics for throughput, AI latency, and error rates to provide real-time visibility into system health.

## 8. Configuration & Resource Management

*   **Configuration**: All system parameters (API keys, paths, limits) are managed via the `config` crate, supporting environment variables and `Settings.toml` files.
    *   **Ingestion Source**: Switch between `NNTP` (live) and `LocalArchive` (filesystem/git) for testing and development.
*   **Git Cleanup**: To prevent disk exhaustion, the Patch Worker implements a "Worktree Garbage Collector" that:
    *   Prunes worktrees immediately after use.
    *   Periodically runs `git gc` on the reference repository.
    *   Enforces a maximum disk usage limit for the scratchpad volume.

## 9. Implementation Roadmap

1.  **Phase 1: Foundation**: NNTP ingestion, basic libSQL schema, and internal task queue.
2.  **Phase 2: Git Ops**: Baseline detection and sandboxed `git am` implementation.
3.  **Phase 3: AI Logic**: Integration with LLM providers and review parsing.
4.  **Phase 4: Web/API**: Axum server and minimalistic HTML/JS frontend.
5.  **Phase 5: Refinement**: Email feedback loop and advanced heuristics.
