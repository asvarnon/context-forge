// TODO(#18): Register ChatHookCommand when chatHooks proposed API stabilizes.
// See https://github.com/asvarnon/context-forge/issues/18
//
// Intended behavior:
// - Register a PreCompact hook pointing to `context-forge-cli pre-compact --db <path>`
// - The CLI binary auto-saves context before Copilot compacts the conversation
// - The chatHooks API currently lacks a programmatic registration method
//
// Implementation will use one of:
// 1. contributes.chatHooks in package.json (no schema documented yet)
// 2. chatSessionCustomizationProvider with ChatSessionCustomizationType.Hook
// 3. User-configured hooks via .agent.md files
