# Licences and provenance

Linger's own code is MIT-licensed (see `../LICENSE`). Everything else that ships inside the
binary is listed here with where it came from and why it may be redistributed.

## Bundled Bible translations — all public domain

Each translation is converted from the eBible.org USFM edition by `cargo run -p xtask -- build-data`.
The download URL and SHA-256 of every source archive are pinned in `xtask/src/main.rs` and recorded
in each translation's metadata (shown in Settings → Translations). Footnotes, cross-references,
Strong's numbers and introductions are dropped; the verse text itself is unchanged.

| Id | Translation | Source (eBible.org id) | Status | Notes |
|---|---|---|---|---|
| `kjv` | King James Version (1769 Oxford standard text) | `eng-kjv2006` | Public domain | Outside the UK. In the United Kingdom the KJV is under perpetual Crown letters patent (printing/importing printed copies needs permission from the Queen's/King's Printers: Cambridge UP, Oxford UP, Collins). eBible.org: "This royal decree has no effect outside of the UK, where this work is firmly in the Public Domain." |
| `web` | World English Bible (2020 stable text, 66 books) | `engwebp` | Public domain | Dedicated to the public domain by its translators. "World English Bible" is a trademark of eBible.org; the name may only be used for unmodified text. Linger does not alter the words (it only omits footnotes), so the name is kept. |
| `bsb` | Berean Standard Bible | `engbsb` | Public domain | Dedicated to the public domain by BSB Publishing / the Berean Bible Translation Committee on April 30, 2023 ("This text of God's Word has been dedicated to the public domain"). |
| `asv` | American Standard Version (1901) | `eng-asv` | Public domain | Published 1901; copyright long expired. |
| `ylt` | Young's Literal Translation (1898) | `engylt` | Public domain | Robert Young; published 1862/1898. |
| `dby` | Darby Translation (1890) | `engDBY` | Public domain | J. N. Darby; published 1867/1890. |
| `wbs` | Webster Bible (1833) | `engwebster` | Public domain | Noah Webster's revision of the KJV. |

eBible.org's catalogue (`translations.csv`) and each archive's `copr.htm` state "Public Domain" for
every source above; copies of those statements are reproduced in [`bibles.md`](bibles.md).

### Considered and not bundled

* **Bible in Basic English** — eBible.org lists it as public domain, but its status outside the
  United States is disputed (Cambridge University Press published the 1965 edition), so it is
  left out.
* **Translations with Apocrypha/Deuterocanon or non-KJV versification** (Douay–Rheims, Brenton
  Septuagint, …) — public domain, but they don't fit the 66-book KJV verse grid used for parallel
  and compare views. Users can import them; deuterocanonical books are skipped.
* **NLT, NASB, LSB, ESV, NIV, NKJV and any other copyrighted translation** — never bundled: no
  text, no derived index, no test fixtures. They can be imported from a file the user owns or
  fetched with the user's own API key (see the README).

## Hebrew and Greek text and lexicon — CC BY 4.0 and public domain

`data/original/original.zst` holds the Hebrew Old Testament and Greek New Testament word by word,
with a lexicon, for Ask's word-study tools (`original_words`, `lexicon`, `word_occurrences`). It is
built by `cargo xtask build-original`; the commit and SHA-256 of every source file are pinned in
`xtask/src/original.rs`.

| Part | Source | Licence |
|---|---|---|
| Tagged Hebrew OT (Leningrad Codex): words, transliteration, English, Strong's numbers, grammar | STEPBible TAHOT, Tyndale House Cambridge | CC BY 4.0 |
| Tagged Greek NT (NA27/28, TR and other editions): the same | STEPBible TAGNT, Tyndale House Cambridge. Its English follows the Berean Study Bible, now in the public domain | CC BY 4.0 |
| Lemmas, transliterations and glosses for every word | STEPBible TBESH (Hebrew) and TBESG (Greek) | CC BY 4.0 |
| Greek definitions | Abbott-Smith, *A Manual Greek Lexicon of the New Testament* (1922), as corrected in STEPBible TBESG | Public domain text; STEPBible's edition CC BY 4.0 |
| Hebrew definitions | Open Scriptures HebrewLexicon: BDB outline (`BrownDriverBriggs.xml`) and Strong's Hebrew dictionary (`HebrewStrong.xml`) | CC BY 4.0; the BDB and Strong's text is public domain |
| Descriptions of grammar codes | STEPBible TEHMC and TEGMC | CC BY 4.0 |

Attribution: "Data created by STEPBible.org based on work at Tyndale House, Cambridge"
(https://www.stepbible.org, https://github.com/STEPBible/STEPBible-Data), and the Open Scriptures
Hebrew Bible Project (https://github.com/openscriptures/HebrewLexicon). Settings shows both.

STEPBible's licence asks that changes be noted. Linger reformats the data and makes these changes:

* Only the words of the 66-book canon on the KJV verse grid. A Psalm title (STEPBible's verse 0)
  joins verse 1, and a Greek word whose KJV verse differs (STEPBible's `[…]` reference) moves to
  it. Hebrew words that STEPBible reconstructs from the Septuagint (type X) and Greek words found in
  neither the modern critical text nor the Greek behind the KJV (type O only) are left out.
* Punctuation tags after `\` in Hebrew are dropped, lexicon HTML becomes plain text, the `(AS)`
  source marks go, all text is NFC-normalized, and BDB's occurrence counts after headwords are
  dropped (the tools count occurrences themselves).
* 26 tags that the lexicon doesn't list (newer sense letters) use the entry of the same number.

Left out, and why:

* **TBESH definitions.** STEPBible says they are Online Bible's Abridged BDB and that permission
  should be gained from Online Bible before they are used. Hebrew definitions come from Open
  Scriptures instead; TBESH supplies only the lemma, transliteration and gloss.
* **Greek definitions from Middle Liddell (ML)** (Perseus, CC BY-SA) and Mounce's Teknia (MT)
  (used by STEPBible with permission). Those words keep STEPBible's gloss without a definition.
* **TFLSJ (full Liddell-Scott-Jones).** It comes from Perseus under CC BY-SA, and it is 24 MB.
* **TAGNT's Spanish and sub-meaning columns** (from the Marvel Bible project and OpenGNT) and
  **TTESV** (CC BY-NC tags for the copyrighted ESV).

## Fonts — SIL Open Font License 1.1

| Font | Use | Source | Licence |
|---|---|---|---|
| Literata (variable, roman + italic) | Scripture text | github.com/google/fonts `ofl/literata` | [OFL-Literata.txt](../apps/desktop/assets/fonts/OFL-Literata.txt) |
| Inter (variable) | Interface | github.com/google/fonts `ofl/inter` | [OFL-Inter.txt](../apps/desktop/assets/fonts/OFL-Inter.txt) |
| Inter Medium, Inter SemiBold (static instances of the variable Inter, made with `fonttools varLib.instancer`, wght 500/600, opsz 14) | Interface, verse numbers | derived from the above | [OFL-Inter.txt](../apps/desktop/assets/fonts/OFL-Inter.txt) |

egui's default fonts (Ubuntu-Light, Hack, Noto Emoji, emoji-icon-font; OFL/Apache/MIT) remain as
fallbacks for symbols.

## Test fixtures

Every importer and online-provider test uses public-domain wording (mostly KJV) in the *format*
of the source being tested (OSIS, Zefania, SWORD, ESV API text, API.Bible JSON). No copyrighted
Bible text appears anywhere in this repository.
