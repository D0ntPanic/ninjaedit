//! Real completion models proposing text in the middle of a line, as
//! when fixing up code, to see how the editor confines what they say.

use std::time::{Duration, Instant};

use ninjaedit_core::{Completer, CompletionOutcome, Editor, FileBuffer, Language};

/// A buffer with `‸` at the cursor.
struct Case {
    language: Language,
    text: &'static str,
}

const CASES: &[Case] = &[
    // Python
    Case {
        language: Language::Python,
        text: "def count_words(path):\n    with open(path) as f:\n        lines = f.readlines()\n    counts = {}\n    for l‸ in lines:\n        for word in line.split():\n            counts[word] = counts.get(word, 0) + 1\n    return counts\n",
    },
    Case {
        language: Language::Python,
        text: "def top_words(objects, n):\n    ranked = sorted(‸, key=lambda x: x[1], reverse=True)\n    return ranked[:n]\n",
    },
    Case {
        language: Language::Python,
        text: "def total(prices, quantities):\n    result = 0\n    for i in range(len(prices)):\n        result += pri‸[i] * quantities[i]\n    return result\n",
    },
    Case {
        language: Language::Python,
        text: "import os\n\ndef list_python_files(root):\n    found = []\n    for dirpath, dirnames, filenames in os.walk(root):\n        for name in filenames:\n            if name.endswith(\".py\"):\n                found.append(os.path.join(dirp‸, name))\n    return found\n",
    },
    Case {
        language: Language::Python,
        text: "class Point:\n    def __init__(self, x, y):\n        self.x = x\n        self.y = y\n\n    def distance(self, other):\n        dx = self.x - other.x\n        dy = self.y - oth‸.y\n        return (dx * dx + dy * dy) ** 0.5\n",
    },
    Case {
        language: Language::Python,
        text: "def parse(line):\n    key, value = line.split(\"=\", 1)\n    return key.st‸(), value.strip()\n",
    },
    // C and C++
    Case {
        language: Language::Cpp,
        text: "#include <iostream>\n#include <string>\n\nvoid greet(const s‸::string& name) {\n    std::cout << \"Hello, \" << name << std::endl;\n}\n",
    },
    Case {
        language: Language::Cpp,
        text: "#include <vector>\n\nint sum(const std::vector<int>& values) {\n    int total = 0;\n    for (size_t i = 0; i < values.size(); i++) {\n        total += val‸[i];\n    }\n    return total;\n}\n",
    },
    Case {
        language: Language::Cpp,
        text: "#include <map>\n#include <string>\n\nint lookup(const std::map<std::string, int>& table, const std::string& key) {\n    auto it = table.fi‸(key);\n    if (it == table.end()) {\n        return -1;\n    }\n    return it->second;\n}\n",
    },
    Case {
        language: Language::C,
        text: "#include <stdio.h>\n#include <string.h>\n\nvoid copy_name(char *dest, const char *src, size_t size) {\n    strncpy(de‸, src, size - 1);\n    dest[size - 1] = '\\0';\n}\n",
    },
    Case {
        language: Language::C,
        text: "#include <stdlib.h>\n\nint *make_array(int count) {\n    int *values = malloc(count * sizeof(*val‸));\n    for (int i = 0; i < count; i++) {\n        values[i] = 0;\n    }\n    return values;\n}\n",
    },
    Case {
        language: Language::Cpp,
        text: "#include <string>\n\nclass Account {\npublic:\n    explicit Account(std::string owner) : owner_(std::move(owner)) {}\n    const std::string& owner() const { return own‸; }\n\nprivate:\n    std::string owner_;\n};\n",
    },
    // Rust
    Case {
        language: Language::Rust,
        text: "use std::collections::HashMap;\n\nfn word_counts(text: &str) -> HashMap<&str, usize> {\n    let mut counts = HashMap::new();\n    for wo‸ in text.split_whitespace() {\n        *counts.entry(word).or_insert(0) += 1;\n    }\n    counts\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn total(prices: &[u32], quantities: &[u32]) -> u32 {\n    let mut sum = 0;\n    for i in 0..prices.len() {\n        sum += pri‸[i] * quantities[i];\n    }\n    sum\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "use std::path::Path;\n\nfn read_config(path: &Path) -> std::io::Result<String> {\n    let text = std::fs::read_to_st‸(path)?;\n    Ok(text.trim().to_owned())\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "struct Point {\n    x: f64,\n    y: f64,\n}\n\nfn distance(a: &Point, b: &Point) -> f64 {\n    let dx = a.x - b.x;\n    let dy = a.y - b‸;\n    (dx * dx + dy * dy).sqrt()\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn largest(values: &[i32]) -> Option<i32> {\n    values.iter().copied().fold(None, |best, v| match be‸ {\n        Some(b) if b >= v => Some(b),\n        _ => Some(v),\n    })\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn first_word(s: &str) -> &str {\n    match s.find(' ') {\n        Some(i) => &s[..‸],\n        None => s,\n    }\n}\n",
    },
    // Inserting in front of what is there, and other shapes.
    Case {
        language: Language::Rust,
        text: "use std::collections::HashMap;\n\nfn tally(words: &[&str]) -> HashMap<String, usize> {\n    let ‸counts = HashMap::new();\n    for w in words {\n        *counts.entry(w.to_string()).or_insert(0) += 1;\n    }\n    counts\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn evens(limit: u32) -> Vec<u32> {\n    let mut out: Vec<‸> = Vec::new();\n    for i in 0..limit {\n        if i % 2 == 0 {\n            out.push(i);\n        }\n    }\n    out\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn mean(values: &[f64]) -> f64 {\n    let total: f64 = val‸.iter().sum();\n    total / values.len() as f64\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "use std::fs::File;\nuse std::io::Read;\n\nfn load(path: &str) -> std::io::Result<String> {\n    let mut file = File::open(pa‸)?;\n    let mut text = String::new();\n    file.read_to_string(&mut text)?;\n    Ok(text)\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn clamp(value: i32, low: i32, high: i32) -> i32 {\n    if value < low {\n        low\n    } else if value > hi‸ {\n        high\n    } else {\n        value\n    }\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn names(people: &[(String, u32)]) -> Vec<String> {\n    people.iter().map(|(name, _)| na‸).collect()\n}\n",
    },
    Case {
        language: Language::Rust,
        text: "fn new_point() -> (i32, i32) {\n    let p = Point { x: ‸, y: 0 };\n    (p.x, p.y)\n}\n",
    },
    Case {
        language: Language::Python,
        text: "def describe(user):\n    name = user[\"name\"]\n    age = user[\"age\"]\n    return {\"name\": na‸, \"age\": age}\n",
    },
    Case {
        language: Language::Python,
        text: "def greet(name):\n    message = f\"Hello, {na‸}!\"\n    print(message)\n",
    },
    Case {
        language: Language::Python,
        text: "def read_lines(path):\n    with open(path, ‸) as f:\n        return f.read().splitlines()\n",
    },
    Case {
        language: Language::Python,
        text: "def scale(values, factor):\n    return [v * fac‸ for v in values]\n",
    },
    Case {
        language: Language::Python,
        text: "def positive(values):\n    return [v for v in values if ‸ > 0]\n",
    },
    Case {
        language: Language::Python,
        text: "class Stack:\n    def __init__(self):\n        self.items = []\n\n    def push(self, item):\n        self.ite‸.append(item)\n",
    },
    Case {
        language: Language::Cpp,
        text: "#include <vector>\n\nstd::vector<int> squares(int n) {\n    std::vector<‸> result;\n    for (int i = 0; i < n; i++) {\n        result.push_back(i * i);\n    }\n    return result;\n}\n",
    },
    Case {
        language: Language::Cpp,
        text: "#include <string>\n#include <iostream>\n\nvoid show(const std::string& label, int value) {\n    std::cout << lab‸ << \": \" << value << std::endl;\n}\n",
    },
    Case {
        language: Language::C,
        text: "#include <stdio.h>\n\nint main(int argc, char **argv) {\n    for (int i = 1; i < ar‸; i++) {\n        printf(\"%s\\n\", argv[i]);\n    }\n    return 0;\n}\n",
    },
    Case {
        language: Language::C,
        text: "#include <stdio.h>\n\nvoid report(const char *name, int count) {\n    printf(\"%s: %d\\n\", ‸, count);\n}\n",
    },
    // Controls: nothing after the cursor on its line but a closing bracket.
    Case {
        language: Language::Rust,
        text: "fn area(width: f64, height: f64) -> f64 {\n    width * height\n}\n\nfn main() {\n    let a = area(‸);\n    println!(\"{a}\");\n}\n",
    },
    Case {
        language: Language::Python,
        text: "def area(width, height):\n    return width * height\n\nprint(area(‸))\n",
    },
];

/// The cursor's line with `inserted` at the cursor.
fn line_with(editor: &Editor, inserted: &str) -> String {
    let text = String::from_utf8(editor.buffer().to_bytes()).unwrap();
    let cursor = editor.cursor();
    let start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
    format!("{}{inserted}{}", &text[start..cursor], &text[cursor..end])
}

/// Each case's completion from the models named, comma-separated, in
/// `NINJAEDIT_COMPLETION_MODELS`: what the model said, and the cursor's
/// line with what the editor suggests of it taken.
#[test]
#[ignore = "needs checkpoints in NINJAEDIT_COMPLETION_MODELS"]
fn in_line_completions_with_real_checkpoints() {
    let models: Vec<String> = std::env::var("NINJAEDIT_COMPLETION_MODELS")
        .expect("comma-separated checkpoint paths")
        .split(',')
        .map(str::to_owned)
        .collect();
    let mut completer = Completer::new();
    completer.set_models(&models);
    for (n, case) in CASES.iter().enumerate() {
        let at = case.text.find('‸').expect("a cursor");
        let mut editor = Editor::new(FileBuffer::from_text(&case.text.replace('‸', "")));
        editor.set_language(case.language);
        editor.set_cursor(at);
        editor.request_completion();
        let request = editor.take_completion_request().unwrap();
        let serial = request.serial;
        completer.request(n as u64, request);
        let started = Instant::now();
        let (text, offered) = loop {
            assert!(started.elapsed() < Duration::from_secs(60), "no answer");
            let answer = completer
                .wait_for_outcomes(Duration::from_secs(1))
                .into_iter()
                .find_map(|outcome| match outcome {
                    CompletionOutcome::Completed {
                        serial: s,
                        text,
                        offered,
                        ..
                    } if s == serial => Some((text, offered)),
                    CompletionOutcome::Failed { model, error } => panic!("{model}: {error}"),
                    _ => None,
                });
            if let Some(answer) = answer {
                break answer;
            }
        };
        editor.offer_completion_clipped(serial, &text, offered);
        let raw_line = text.split('\n').next().unwrap_or_default();
        println!("{:?} {:?}", case.language, line_with(&editor, "‸"));
        println!("  model:     {:?} (offering {})", text, offered);
        println!("  unclipped: {:?}", line_with(&editor, raw_line));
        println!(
            "  suggested: {:?} -> {:?}",
            editor.suggestion().unwrap_or_default(),
            line_with(&editor, editor.suggestion().unwrap_or_default())
        );
        // What the editor keeps of all the model said, offered or not.
        let full = editor.full_suggestion().unwrap_or_default();
        let full_line = full.split('\n').next().unwrap_or_default();
        println!(
            "  confined:  {:?} -> {:?}",
            full,
            line_with(&editor, full_line)
        );
        println!();
    }
}
