//! Prompt template for bootstrapping a persona-specific [`super::ConfigLexiconScorer`].

/// Generate a calibration prompt for bootstrapping a persona lexicon via an LLM.
///
/// Pass the returned string to any LLM. The model's response will be a fenced TOML
/// block suitable for parsing with [`super::ConfigLexiconScorer::from_str`] or saving
/// to disk for [`super::ConfigLexiconScorer::from_file`].
///
/// CF provides the calibration baseline; the caller is responsible for making the
/// LLM call and persisting the result. Extract the TOML from inside the fenced block
/// before parsing.
///
/// # Example
///
/// ```
/// use context_forge::lexicon::bootstrap_prompt;
///
/// let prompt = bootstrap_prompt("A Space Marine Chaplain from Warhammer 40k");
/// assert!(prompt.contains("Space Marine Chaplain"));
/// assert!(prompt.contains("0.0, 1.5]"));
/// ```
#[must_use]
pub fn bootstrap_prompt(persona_description: &str) -> String {
    format!(
        r#"You are generating a lexicon configuration for a memory importance scoring system.

The AI assistant using this lexicon has the following persona:
<persona>
{persona_description}
</persona>

## What this lexicon does

This lexicon teaches a deterministic scoring system which domain-specific terms and phrases
signal "this conversation entry is worth remembering." Entries that score higher survive a
token budget cut and are surfaced in future conversations.

The scoring formula is:
  final_score = base_score × (1.0 + boost.clamp(-1.0, 2.0))

Where boost accumulates as follows:
  - Each matched [terms] entry adds its weight directly to boost
  - Each matched [affirmations] pattern adds +0.5 to boost
  - Each matched [negations] pattern subtracts 0.3 from boost

A boost of 0.0 leaves the score unchanged. A boost of 1.0 doubles it (2.0×).
The engine caps total boost at 2.0, giving a 3.0× maximum multiplier.

## Weight calibration

| Range     | Use for                                                                          |
|-----------|----------------------------------------------------------------------------------|
| 0.1–0.4   | Mildly domain-specific. Appears in casual and important content alike.           |
| 0.5–0.8   | Strongly domain-specific. More often in important entries than not.              |
| 0.9–1.5   | Critical term or proper noun. Almost always marks high-value content.            |

Weights must be in (0.0, 1.5]. Never assign a weight above 1.5; the library will
reject any config that does.

## Inclusion rules for [terms]

1. Minimum 4 characters, unless the term is a well-known domain acronym.
2. Prefer precise multi-word phrases over short, ambiguous single words.
3. Memory-value test: include a term ONLY if its presence in an entry makes that entry
   meaningfully more likely to be worth recalling later. Do not include terms merely
   because they sound authentic or in-character for the persona.

## What NOT to include

The system already handles generic English signals ("confirmed", "agreed", "remember this",
"never mind", "my mistake", "incorrect", and similar). Do not repeat them. Only
domain-specific vocabulary and dialect belong in this lexicon.

## [affirmations] — speech act rules

Affirmation patterns must map to one of these speech acts in this persona's dialect:
  - Agreement or confirmation
  - Future commitment or obligation
  - Success or resolution
  - Flagging something as important or worth noting

Aim for 6–12 patterns. Domain-specific dialect only — no generic English.

## [negations] — speech act rules

Negation patterns must map to one of these speech acts in this persona's dialect:
  - Dismissal or disregard
  - Disagreement or correction
  - Failure or rejection

Aim for 4–8 patterns. Domain-specific dialect only — no generic English.

## Output instructions

Think through the calibration internally before writing any output. Reason about which
terms are genuinely high-signal vs. merely in-character, and what speech acts this
persona's dialect uses to express agreement, commitment, dismissal, and failure.

Then output ONLY a single fenced TOML block. No markdown, no prose before or after
the block. Put short rationale as valid TOML inline comments.

```toml
# Persona lexicon — generated for context-forge
# Persona: {persona_description}

[terms]
"term" = 0.4   # rationale: why this term signals important content

[affirmations]
patterns = [
    "phrase",   # speech act: confirmation
]

[negations]
patterns = [
    "phrase",   # speech act: dismissal
]
```"#,
    )
}
