//! Dubeolsik, the standard two-set Korean keyboard: the keys that type a piece of Hangul text,
//! so text the phone's input method composed can be composed again by the computer's.

/// The 19 initial consonants, in Unicode order.
const INITIALS: [&str; 19] = [
    "r", "R", "s", "e", "E", "f", "a", "q", "Q", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g",
];

/// The 21 vowels, in Unicode order. Compound vowels take two keys.
const VOWELS: [&str; 21] = [
    "k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b",
    "m", "ml", "l",
];

/// The final consonants, in Unicode order after "none".
const FINALS: [&str; 28] = [
    "", "r", "R", "rt", "s", "sw", "sg", "e", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a",
    "q", "qt", "t", "T", "d", "w", "c", "z", "x", "v", "g",
];

/// Compatibility jamo U+3131..=U+314E. A double final on its own (ㄳ) cannot be typed: the
/// input method commits the first consonant before the second arrives.
const CONSONANTS: [Option<&str>; 30] = [
    Some("r"),
    Some("R"),
    None,
    Some("s"),
    None,
    None,
    Some("e"),
    Some("E"),
    Some("f"),
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some("a"),
    Some("q"),
    Some("Q"),
    None,
    Some("t"),
    Some("T"),
    Some("d"),
    Some("w"),
    Some("W"),
    Some("c"),
    Some("z"),
    Some("x"),
    Some("v"),
    Some("g"),
];

const SYLLABLES: std::ops::RangeInclusive<u32> = 0xac00..=0xd7a3;
const CONSONANT_JAMO: std::ops::RangeInclusive<u32> = 0x3131..=0x314e;
const VOWEL_JAMO: std::ops::RangeInclusive<u32> = 0x314f..=0x3163;

/// Keys for one modern Hangul syllable or compatibility jamo.
pub fn keys(c: char) -> Option<String> {
    let cp = c as u32;
    if SYLLABLES.contains(&cp) {
        let s = (cp - SYLLABLES.start()) as usize;
        Some([INITIALS[s / 588], VOWELS[s % 588 / 28], FINALS[s % 28]].concat())
    } else if CONSONANT_JAMO.contains(&cp) {
        CONSONANTS[(cp - CONSONANT_JAMO.start()) as usize].map(str::to_owned)
    } else if VOWEL_JAMO.contains(&cp) {
        Some(VOWELS[(cp - VOWEL_JAMO.start()) as usize].to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllables() {
        assert_eq!(keys('가').as_deref(), Some("rk"));
        assert_eq!(keys('한').as_deref(), Some("gks"));
        assert_eq!(keys('글').as_deref(), Some("rmf"));
        assert_eq!(keys('닭').as_deref(), Some("ekfr"));
        assert_eq!(keys('꽉').as_deref(), Some("Rhkr"));
        assert_eq!(keys('읽').as_deref(), Some("dlfr"));
        assert_eq!(keys('힣').as_deref(), Some("glg"));
        assert_eq!(keys('뷁').as_deref(), Some("qnpfr"));
    }

    #[test]
    fn jamo() {
        assert_eq!(keys('ㄱ').as_deref(), Some("r"));
        assert_eq!(keys('ㅎ').as_deref(), Some("g"));
        assert_eq!(keys('ㅃ').as_deref(), Some("Q"));
        assert_eq!(keys('ㅏ').as_deref(), Some("k"));
        assert_eq!(keys('ㅘ').as_deref(), Some("hk"));
        assert_eq!(keys('ㅣ').as_deref(), Some("l"));
        assert_eq!(keys('ㄳ'), None);
        assert_eq!(keys('ㅄ'), None);
    }

    #[test]
    fn others() {
        assert_eq!(keys('a'), None);
        assert_eq!(keys('é'), None);
        assert_eq!(keys('ㆍ'), None);
        assert_eq!(keys('漢'), None);
    }
}
