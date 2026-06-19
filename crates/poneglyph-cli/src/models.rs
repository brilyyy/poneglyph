//! Curated embedding model presets offered at `poneglyph init`, spanning
//! multilingual to English-only. All confirmed `BertModel`/384-dim against
//! each model's real HF `config.json`, so they drop into the existing
//! `embed_anything::from_pretrained_hf` (candle) path with no other code
//! changes — see the plan notes on why architecture (not just dimension)
//! has to be checked before a model goes in this list.

pub struct ModelPreset {
    pub id: &'static str,
    pub pros: &'static str,
    pub cons: &'static str,
}

pub const PRESETS: &[ModelPreset] = &[
    ModelPreset {
        id: "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2",
        pros: "50+ languages, balanced quality/speed (default)",
        cons: "Slightly behind English-specialized models on English-only corpora",
    },
    ModelPreset {
        id: "BAAI/bge-small-en-v1.5",
        pros: "Best-in-class small English retrieval quality",
        cons: "English only",
    },
    ModelPreset {
        id: "sentence-transformers/all-MiniLM-L6-v2",
        pros: "Smallest/fastest, huge ecosystem precedent",
        cons: "Lowest retrieval quality of the three, English only",
    },
];

/// Resolve a raw stdin line to a preset id. Blank, unparsable, or
/// out-of-range input all fall back to the first (default) preset, so a
/// fumbled keystroke can't leave `init` in a half-configured state.
fn parse_choice(input: &str) -> &'static str {
    let choice: usize = input.trim().parse().unwrap_or(1);
    PRESETS.get(choice.saturating_sub(1)).map(|p| p.id).unwrap_or(PRESETS[0].id)
}

fn print_menu() {
    use std::io::Write;
    println!("\nChoose an embedding model (used for semantic recall):");
    for (i, p) in PRESETS.iter().enumerate() {
        println!("  {}. {}", i + 1, p.id);
        println!("     + {}", p.pros);
        println!("     - {}", p.cons);
    }
    print!("Enter 1-{} [1]: ", PRESETS.len());
    let _ = std::io::stdout().flush();
}

/// Interactively pick a model. Non-TTY stdin (CI/scripts/pipes) skips the
/// prompt entirely and returns the first preset — `init` stays script-safe.
pub fn pick_model() -> &'static str {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return PRESETS[0].id;
    }

    print_menu();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return PRESETS[0].id;
    }
    parse_choice(&line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_choice_picks_by_number() {
        assert_eq!(parse_choice("1"), PRESETS[0].id);
        assert_eq!(parse_choice("2"), PRESETS[1].id);
        assert_eq!(parse_choice("3\n"), PRESETS[2].id);
    }

    #[test]
    fn parse_choice_falls_back_to_first_on_bad_input() {
        assert_eq!(parse_choice(""), PRESETS[0].id);
        assert_eq!(parse_choice("abc"), PRESETS[0].id);
        assert_eq!(parse_choice("99"), PRESETS[0].id);
        assert_eq!(parse_choice("0"), PRESETS[0].id);
    }
}
