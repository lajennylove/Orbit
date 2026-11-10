//! The default table name, as Laravel's Eloquent builds it: `Str::snake(Str::pluralStudly(name))`.
//! Only the last word is pluralized, so `UserProfile` gives `user_profiles`.
//!
//! The plural follows Laravel's `Pluralizer`, which uses doctrine/inflector's English rules
//! (2.1.x). The word lists and rules below are copied from that source, in its order.

/// The default table name for a struct named `name`: `Flight` gives `flights`, `UserProfile`
/// gives `user_profiles`.
pub(crate) fn table_name(name: &str) -> String {
    let (head, last) = split_last_word(name);
    snake(&format!("{head}{}", plural(last)))
}

/// Splits `UserProfile` into `("User", "Profile")`. The last word starts at the last capital
/// letter that follows another character.
fn split_last_word(name: &str) -> (&str, &str) {
    let cut = name
        .char_indices()
        .skip(1)
        .filter(|(_, c)| c.is_ascii_uppercase())
        .last()
        .map_or(0, |(i, _)| i);
    name.split_at(cut)
}

/// Inserts `_` before each capital letter that follows another character, then lowercases.
fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// The plural of one English word, as Laravel's `Str::plural` gives it. `Flight` gives
/// `Flights`, `Category` gives `Categories`, `Person` gives `People`.
///
/// Limits: the rules are ASCII only, so a word that ends in a non-ASCII letter takes a plain
/// `s`. The rules cover the common English patterns, not every word; a word that matches no
/// rule gets an `s`.
pub(crate) fn plural(word: &str) -> String {
    // Laravel leaves a word alone when it ends in a character that is not a letter or digit.
    let Some(last) = word.chars().last() else {
        return String::new();
    };
    if !(last.is_ascii_alphanumeric() || last >= '\u{80}') {
        return word.to_owned();
    }
    let lower = word.to_ascii_lowercase();
    if UNCOUNTABLE.contains(&lower.as_str()) {
        return word.to_owned();
    }
    match_case(plural_lower(&lower), word)
}

/// The plural of a lowercase word. Uninflected words win, then irregular ones, then the first
/// suffix rule that matches.
fn plural_lower(word: &str) -> String {
    if is_uninflected(word) {
        return word.to_owned();
    }
    if let Some((_, plural)) = IRREGULAR.iter().find(|(singular, _)| *singular == word) {
        return (*plural).to_owned();
    }
    RULES
        .iter()
        .find_map(|rule| rule.apply(word))
        .unwrap_or_else(|| word.to_owned())
}

/// Words that are the same in the plural, such as `equipment` and `sheep`.
const UNCOUNTABLE: &[&str] = &["recommended", "related"];

/// Whether a word is the same in the plural: an exact word from the list, or a word made of
/// `\w+` followed by `media` or `ware`.
fn is_uninflected(word: &str) -> bool {
    UNINFLECTED.contains(&word)
        || word_before_suffix(word, "media")
        || word_before_suffix(word, "ware")
}

/// Whether `word` is `\w+` followed by `suffix`, so at least one word character comes first.
fn word_before_suffix(word: &str, suffix: &str) -> bool {
    word.len() > suffix.len()
        && word.ends_with(suffix)
        && word.as_bytes()[..word.len() - suffix.len()]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// One suffix rule. The rules run in order, and the first one that matches the end of the word
/// is used.
enum Rule {
    /// The word ends with `.0`, and that ending becomes `.1`.
    Suffix(&'static str, &'static str),
    /// `y` after a consonant, or after `qu`, becomes `ies`.
    ConsonantY,
    /// `fe` after a consonant, or `f` after `l` or `r`, becomes `ves`.
    FeOrF,
}

impl Rule {
    fn apply(&self, word: &str) -> Option<String> {
        match self {
            Rule::Suffix(from, to) => word.strip_suffix(*from).map(|stem| format!("{stem}{to}")),
            Rule::ConsonantY => {
                let stem = word.strip_suffix('y')?;
                let prev = stem.as_bytes().last()?;
                let consonant = !b"aeiouy".contains(prev) || stem.ends_with("qu");
                consonant.then(|| format!("{stem}ies"))
            }
            Rule::FeOrF => {
                let bytes = word.as_bytes();
                if let Some(stem) = word.strip_suffix("fe")
                    && bytes.len() >= 3
                    && bytes[bytes.len() - 3] != b'f'
                {
                    return Some(format!("{stem}ves"));
                }
                word.strip_suffix('f')
                    .filter(|stem| stem.ends_with('l') || stem.ends_with('r'))
                    .map(|stem| format!("{stem}ves"))
            }
        }
    }
}

/// The suffix rules, in doctrine/inflector's order.
const RULES: &[Rule] = &[
    Rule::Suffix("status", "statuses"),
    Rule::Suffix("quiz", "quizzes"),
    Rule::Suffix("mouse", "mice"),
    Rule::Suffix("louse", "lice"),
    Rule::Suffix("matrix", "matrices"),
    Rule::Suffix("matrex", "matrices"),
    Rule::Suffix("vertix", "vertices"),
    Rule::Suffix("vertex", "vertices"),
    Rule::Suffix("indix", "indices"),
    Rule::Suffix("index", "indices"),
    Rule::Suffix("x", "xes"),
    Rule::Suffix("ch", "ches"),
    Rule::Suffix("ss", "sses"),
    Rule::Suffix("sh", "shes"),
    Rule::ConsonantY,
    Rule::Suffix("hive", "hives"),
    Rule::Suffix("gulf", "gulfs"),
    Rule::FeOrF,
    Rule::Suffix("sis", "ses"),
    Rule::Suffix("tum", "ta"),
    Rule::Suffix("ium", "ia"),
    Rule::Suffix("taxon", "taxa"),
    Rule::Suffix("criterion", "criteria"),
    Rule::Suffix("person", "people"),
    Rule::Suffix("man", "men"),
    Rule::Suffix("child", "children"),
    Rule::Suffix("foot", "feet"),
    Rule::Suffix("buffalo", "buffaloes"),
    Rule::Suffix("hero", "heroes"),
    Rule::Suffix("potato", "potatoes"),
    Rule::Suffix("tomato", "tomatoes"),
    Rule::Suffix("volcano", "volcanoes"),
    Rule::Suffix("alumnus", "alumni"),
    Rule::Suffix("bacillus", "bacilli"),
    Rule::Suffix("cactus", "cacti"),
    Rule::Suffix("focus", "foci"),
    Rule::Suffix("fungus", "fungi"),
    Rule::Suffix("nucleus", "nuclei"),
    Rule::Suffix("radius", "radii"),
    Rule::Suffix("stimulus", "stimuli"),
    Rule::Suffix("syllabus", "syllabi"),
    Rule::Suffix("terminus", "termini"),
    Rule::Suffix("virus", "viri"),
    Rule::Suffix("us", "uses"),
    Rule::Suffix("alias", "aliases"),
    Rule::Suffix("analysis", "analyses"),
    Rule::Suffix("axis", "axes"),
    Rule::Suffix("crisis", "crises"),
    Rule::Suffix("thesis", "theses"),
    Rule::Suffix("testis", "testes"),
    Rule::Suffix("s", "s"),
    Rule::Suffix("", "s"),
];

/// Gives the plural the capitals of the word it came from, as Laravel's `matchCase` does.
fn match_case(plural: String, word: &str) -> String {
    if !word.chars().any(|c| c.is_uppercase()) {
        return plural.to_lowercase();
    }
    if !word.chars().any(|c| c.is_lowercase()) {
        return plural.to_uppercase();
    }
    if word.chars().next().is_some_and(|c| !c.is_lowercase()) {
        let mut chars = plural.chars();
        return match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => plural,
        };
    }
    plural
}

/// Irregular singulars and their plurals, from doctrine/inflector. Each one matches the whole
/// word.
const IRREGULAR: &[(&str, &str)] = &[
    ("abuse", "abuses"),
    ("alga", "algae"),
    ("atlas", "atlases"),
    ("avalanche", "avalanches"),
    ("axis", "axes"),
    ("axe", "axes"),
    ("beef", "beefs"),
    ("blouse", "blouses"),
    ("brother", "brothers"),
    ("brownie", "brownies"),
    ("cache", "caches"),
    ("cafe", "cafes"),
    ("canvas", "canvases"),
    ("cave", "caves"),
    ("chateau", "chateaux"),
    ("child", "children"),
    ("cookie", "cookies"),
    ("corpus", "corpuses"),
    ("cow", "cows"),
    ("criterion", "criteria"),
    ("curriculum", "curricula"),
    ("curve", "curves"),
    ("demo", "demos"),
    ("die", "dice"),
    ("domino", "dominoes"),
    ("echo", "echoes"),
    ("emphasis", "emphases"),
    ("epoch", "epochs"),
    ("foe", "foes"),
    ("foot", "feet"),
    ("fungus", "fungi"),
    ("ganglion", "ganglions"),
    ("gas", "gases"),
    ("genie", "genies"),
    ("genus", "genera"),
    ("goose", "geese"),
    ("graffito", "graffiti"),
    ("grave", "graves"),
    ("hippopotamus", "hippopotami"),
    ("hoax", "hoaxes"),
    ("hoof", "hoofs"),
    ("human", "humans"),
    ("iris", "irises"),
    ("larva", "larvae"),
    ("leaf", "leaves"),
    ("lens", "lenses"),
    ("loaf", "loaves"),
    ("man", "men"),
    ("medium", "media"),
    ("memorandum", "memoranda"),
    ("money", "monies"),
    ("mongoose", "mongooses"),
    ("motto", "mottoes"),
    ("move", "moves"),
    ("mythos", "mythoi"),
    ("neurosis", "neuroses"),
    ("niche", "niches"),
    ("niveau", "niveaux"),
    ("nucleus", "nuclei"),
    ("numen", "numina"),
    ("nursery", "nurseries"),
    ("oasis", "oases"),
    ("occiput", "occiputs"),
    ("octopus", "octopuses"),
    ("opus", "opuses"),
    ("ox", "oxen"),
    ("passerby", "passersby"),
    ("penis", "penises"),
    ("person", "people"),
    ("plateau", "plateaux"),
    ("runner-up", "runners-up"),
    ("safe", "safes"),
    ("save", "saves"),
    ("sex", "sexes"),
    ("sieve", "sieves"),
    ("soliloquy", "soliloquies"),
    ("son-in-law", "sons-in-law"),
    ("stadium", "stadiums"),
    ("syllabus", "syllabi"),
    ("testis", "testes"),
    ("thief", "thieves"),
    ("tooth", "teeth"),
    ("tornado", "tornadoes"),
    ("trilby", "trilbys"),
    ("turf", "turfs"),
    ("valve", "valves"),
    ("volcano", "volcanoes"),
    ("wave", "waves"),
    ("zombie", "zombies"),
];

/// Words with no plural form, from doctrine/inflector. `\w+media` and `\w+ware` are matched in
/// `is_uninflected`, not listed here.
const UNINFLECTED: &[&str] = &[
    "advice",
    "aircraft",
    "amoyese",
    "art",
    "audio",
    "baggage",
    "bison",
    "borghese",
    "bream",
    "breeches",
    "britches",
    "buffalo",
    "butter",
    "cantus",
    "carp",
    "cattle",
    "chassis",
    "clippers",
    "clothing",
    "coal",
    "cod",
    "coitus",
    "compensation",
    "congoese",
    "contretemps",
    "coreopsis",
    "corps",
    "cotton",
    "data",
    "debris",
    "deer",
    "diabetes",
    "djinn",
    "education",
    "eland",
    "elk",
    "emoji",
    "equipment",
    "evidence",
    "faroese",
    "feedback",
    "fish",
    "flounder",
    "flour",
    "foochowese",
    "food",
    "furniture",
    "gallows",
    "genevese",
    "genoese",
    "gilbertese",
    "gold",
    "headquarters",
    "herpes",
    "hijinks",
    "homework",
    "hottentotese",
    "impatience",
    "information",
    "innings",
    "jackanapes",
    "jeans",
    "jedi",
    "kin",
    "kiplingese",
    "knowledge",
    "kongoese",
    "leather",
    "love",
    "lucchese",
    "luggage",
    "mackerel",
    "maltese",
    "management",
    "media",
    "metadata",
    "mews",
    "money",
    "moose",
    "mumps",
    "music",
    "nankingese",
    "news",
    "nexus",
    "niasese",
    "nutrition",
    "offspring",
    "oil",
    "patience",
    "pekingese",
    "people",
    "piedmontese",
    "pincers",
    "pistoiese",
    "plankton",
    "pliers",
    "pokemon",
    "police",
    "polish",
    "portuguese",
    "proceedings",
    "progress",
    "rabies",
    "rain",
    "research",
    "rhinoceros",
    "rice",
    "salmon",
    "sand",
    "sarawakese",
    "scissors",
    "series",
    "shavese",
    "shears",
    "sheep",
    "siemens",
    "silk",
    "sms",
    "soap",
    "social media",
    "spam",
    "species",
    "staff",
    "sugar",
    "swine",
    "talent",
    "toothpaste",
    "traffic",
    "travel",
    "trivia",
    "trousers",
    "trout",
    "tuna",
    "us",
    "vermontese",
    "vinegar",
    "weather",
    "wenchowese",
    "wheat",
    "whiting",
    "wildebeest",
    "wood",
    "wool",
    "yengeese",
    "sea bass",
    "sea-bass",
];

#[cfg(test)]
mod tests {
    use super::{plural, table_name};

    #[test]
    fn plural_adds_s_by_default() {
        assert_eq!(plural("Flight"), "Flights");
        assert_eq!(plural("Post"), "Posts");
        assert_eq!(plural("Day"), "Days");
        assert_eq!(plural("Queue"), "Queues");
    }

    #[test]
    fn plural_follows_the_suffix_rules() {
        assert_eq!(plural("Category"), "Categories");
        assert_eq!(plural("Address"), "Addresses");
        assert_eq!(plural("Box"), "Boxes");
        assert_eq!(plural("Bus"), "Buses");
        assert_eq!(plural("Quiz"), "Quizzes");
        assert_eq!(plural("Status"), "Statuses");
        assert_eq!(plural("Alias"), "Aliases");
        assert_eq!(plural("Knife"), "Knives");
        assert_eq!(plural("Wolf"), "Wolves");
        assert_eq!(plural("Leaf"), "Leaves");
        assert_eq!(plural("Hero"), "Heroes");
        assert_eq!(plural("Potato"), "Potatoes");
        assert_eq!(plural("Mouse"), "Mice");
        assert_eq!(plural("Matrix"), "Matrices");
        assert_eq!(plural("Index"), "Indices");
        assert_eq!(plural("Analysis"), "Analyses");
        assert_eq!(plural("Crisis"), "Crises");
        assert_eq!(plural("Fungus"), "Fungi");
        assert_eq!(plural("Datum"), "Data");
        assert_eq!(plural("Criterion"), "Criteria");
        assert_eq!(plural("Chef"), "Chefs");
    }

    #[test]
    fn plural_uses_irregular_words_as_whole_words() {
        assert_eq!(plural("Person"), "People");
        assert_eq!(plural("Child"), "Children");
        assert_eq!(plural("Man"), "Men");
        assert_eq!(plural("Woman"), "Women");
        assert_eq!(plural("Foot"), "Feet");
        assert_eq!(plural("Goose"), "Geese");
        assert_eq!(plural("Tooth"), "Teeth");
        assert_eq!(plural("Ox"), "Oxen");
    }

    #[test]
    fn plural_keeps_uncountable_words() {
        assert_eq!(plural("Equipment"), "Equipment");
        assert_eq!(plural("Information"), "Information");
        assert_eq!(plural("Sheep"), "Sheep");
        assert_eq!(plural("Fish"), "Fish");
        assert_eq!(plural("Series"), "Series");
        assert_eq!(plural("Species"), "Species");
        assert_eq!(plural("Money"), "Money");
        assert_eq!(plural("News"), "News");
        assert_eq!(plural("Related"), "Related");
        assert_eq!(plural("Software"), "Software");
    }

    #[test]
    fn plural_keeps_words_doctrine_lists_as_plural_forms() {
        // Doctrine's plural list has these as uninflected. `Maltese` was once stored with a
        // capital, so it never matched the lowercased word.
        assert_eq!(plural("Media"), "Media");
        assert_eq!(plural("Multimedia"), "Multimedia");
        assert_eq!(plural("People"), "People");
        assert_eq!(plural("Trivia"), "Trivia");
        assert_eq!(plural("Maltese"), "Maltese");
        assert_eq!(table_name("SocialMedia"), "social_media");
        assert_eq!(table_name("Trivia"), "trivia");
    }

    #[test]
    fn plural_follows_the_testis_rule() {
        assert_eq!(plural("Testis"), "Testes");
        assert_eq!(table_name("SpermTestis"), "sperm_testes");
    }

    #[test]
    fn plural_matches_the_case_of_the_word() {
        assert_eq!(plural("box"), "boxes");
        assert_eq!(plural("BOX"), "BOXES");
        assert_eq!(plural("Box"), "Boxes");
    }

    #[test]
    fn table_name_pluralizes_only_the_last_word() {
        assert_eq!(table_name("Flight"), "flights");
        assert_eq!(table_name("UserProfile"), "user_profiles");
        assert_eq!(table_name("Category"), "categories");
        assert_eq!(table_name("Address"), "addresses");
        assert_eq!(table_name("Person"), "people");
        assert_eq!(table_name("Child"), "children");
        assert_eq!(table_name("Box"), "boxes");
        assert_eq!(table_name("Status"), "statuses");
        assert_eq!(table_name("OrderLineItem"), "order_line_items");
        assert_eq!(table_name("SalesPerson"), "sales_people");
    }

    #[test]
    fn table_name_splits_acronyms_as_laravel_does() {
        assert_eq!(table_name("HTTPRequest"), "h_t_t_p_requests");
    }
}
