# Sashiko Development and CI Tasks

.PHONY: help check-pr check-integration check-all sob lint test integration-test

# Default target
.DEFAULT_GOAL := help

# List available commands
help:
	@echo "Available targets:"
	@echo "  help              - List available commands"
	@echo "  check-pr          - [PR Suite] Run all checks required for a Pull Request (SOB, Lint, Unit Tests)"
	@echo "  check-integration - [Integration Suite] Run the full integration tests"
	@echo "  check-all         - Run the complete check suite (PR + Integration)"
	@echo "  sob               - Check Signed-off-by tags (default: HEAD~1..HEAD). Use RANGE to override."
	@echo "  lint              - Run all linters (clippy, fmt, yamllint)"
	@echo "  test              - Run unit tests"
	@echo "  integration-test  - [Slow] Run integration tests using benchmarks"

# [PR Suite] Run all checks required for a Pull Request (SOB, Lint, Unit Tests)
check-pr: sob lint test

# [Integration Suite] Run the full integration tests
check-integration: integration-test

# Run the complete check suite (PR + Integration)
check-all: check-pr check-integration

# Check Signed-off-by tags (default: HEAD~1..HEAD)
RANGE ?= HEAD~1..HEAD
sob:
	@./scripts/check-sob.sh "$(RANGE)"

# Run all linters (clippy, fmt, yamllint)
lint:
	@cargo clippy --all-targets --all-features --release -- -D warnings
	@cargo fmt --all -- --check
	@yamllint .

# Run unit tests
test:
	@cargo test --release

# [Slow] Run integration tests using benchmarks
integration-test:
	@./scripts/integration-test.sh
