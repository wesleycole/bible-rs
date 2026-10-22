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

## Cross-references — CC BY

`data/crossrefs/crossrefs.zst` holds the cross-references Ask's `cross_references` tool reads. It is
built by `cargo xtask build-crossrefs` from OpenBible.info's cross-reference file
(https://a.openbible.info/data/cross-references.zip), whose SHA-256 is pinned in
`xtask/src/crossrefs.rs`. OpenBible.info regenerates the file daily as readers vote, so a rebuild
takes the current file (`XTASK_ACCEPT_UPDATED_SOURCES=1`) and updates the pin.

| Part | Source | Licence |
|---|---|---|
| About 340,000 links between verses, each with readers' net votes | OpenBible.info, starting from the Treasury of Scripture Knowledge (public domain) | CC BY ("www.openbible.info CC-BY" in the file) |

Attribution: "Cross references from OpenBible.info" (https://www.openbible.info/labs/cross-references/).
Settings shows it.

Changes: links with no net votes in their favour (about 3,500) are left out; references are mapped
onto the KJV verse grid, leaving out the one link whose source verse isn't on it (3 John 1:15) and
stopping four passages at the end of a KJV chapter.

## People and places — CC BY 4.0

`data/names/names.zst` holds every person, place and other proper name in the Bible, each
individual told apart, for Ask's `people_and_places` tool. It is built by `cargo xtask build-names`;
the commit and SHA-256 of every source file are pinned in `xtask/src/names.rs`.

| Part | Source | Licence |
|---|---|---|
| People, places and other names: each individual's forms of the name in English, Hebrew and Greek with Strong's numbers, every reference, family and tribe, a place's founders and inhabitants | STEPBible TIPNR, Tyndale House Cambridge | CC BY 4.0 |
| The modern locations identified with each place, the confidence of scholarship in each, and their coordinates | OpenBible.info Bible Geocoding Data (`ancient.jsonl`, `modern.jsonl`) | CC BY 4.0 |

Attribution: "Data created by STEPBible.org based on work at Tyndale House, Cambridge"
(https://www.stepbible.org) and "Bible geocoding data from OpenBible.info"
(https://www.openbible.info/geo/). Settings shows both.

STEPBible's licence asks that changes be noted. Linger reformats the data and makes these changes:

* Each person or place's id is its name and first reference without the last book TIPNR appends
  (`Aaron@Exo.4.14`, not `Aaron@Exo.4.14-Heb`); TIPNR's relation marks become words: `(d)` and
  `(a)` "(ancestor)", `(f)` "(founder)", `(?)` "(uncertain)".
* Lines that repeat a form of a name for a textual variant join that form. References are mapped
  onto the KJV verse grid, and a second mention in a verse (`Exo.7.10b`) counts once.
* Two relations that name no entity in TIPNR (the Maacathites as ancestor of Hodiah and Ishbah)
  are left out, as are three entries named in no verse once Septuagint-only references are gone.
* Each place takes OpenBible.info's identifications for the place TIPNR names in its "OpenBible
  name" column, or failing that the place whose TIPNR link names it. Identifications with no
  support in current scholarship (a time-weighted score of 0 or less) are left out; a score out of
  1,000 becomes a confidence out of 100.

Left out, and why:

* **TIPNR's `@Briefest`, `@Brief`, `@Short` and `@Article` descriptions.** STEPBible says they were
  adapted from an AI model's output (Claude 3 Opus), and one checked against the text was wrong.
  Ask is meant to answer from sources, not from another model's summary.
* **Septuagint-only references** (marked `LXX`): the text Linger reads has no Septuagint.
* **Coordinates OpenBible.info took from OpenStreetMap** (ODbL, a share-alike licence; about 50
  identifications), and all of OpenBible.info's geometry and images. Those identifications are
  kept without coordinates.

## Outside sources — public domain

`data/sources/<work>.jsonl.zst` holds the commentaries, reference works and confessions Ask draws on
(docs/plans/m7-outside-sources.md). They are not compiled into the apps: `server/scripts/load-sources.ts`
loads them into shared, read-only Convex tables. Each is built by `cargo xtask build-sources` from a
CrossWire SWORD module (`https://www.crosswire.org/ftpmirror/pub/sword/packages/rawzip/<Module>.zip`)
whose SHA-256 is pinned in `xtask/src/sources.rs`; every module's conf says
`DistributionLicense=Public Domain`. The module is the pinned edition; its own source is given so the
text can be checked against it.

| Id | Work | Module | Module's source | Why public domain |
|---|---|---|---|---|
| `mhc` | Matthew Henry, *Commentary on the Whole Bible* (1708–1710) | `MHC` 2.2 | CCEL | Henry died in 1714. |
| `jfb` | Jamieson, Fausset and Brown, *Commentary Critical and Explanatory on the Whole Bible* (1871) | `JFB` 3.0 | CCEL (`ccel.org/ccel/j/jamieson/jfb`) | Jamieson died in 1880, Brown in 1897, Fausset in 1910. |
| `barnes` | Albert Barnes, *Notes on the New Testament* (1832–1851) | `Barnes` 1.1 | CrossWire's own ThML edition | Barnes died in 1870. The module holds the New Testament only; his Old Testament notes are not in it. |
| `kd` | Keil and Delitzsch, *Biblical Commentary on the Old Testament* (English edition, T&T Clark, 1864–1891) | `KD` 2.0 | Wikisource | Keil died in 1888, Delitzsch in 1890. The English edition was published before 1929, so it is public domain in the United States, and Wikisource and CrossWire both distribute it as public domain; the translators' dates are not all recorded, which is the one open point. Old Testament only, as the work is. |
| `calvin` | John Calvin, *Commentaries* (Calvin Translation Society English editions, 1844–1856) | `CalvinCommentaries` 1.1 | CCEL | Calvin died in 1564; the Society's translators (Beveridge, King, Owen, Pringle and others) died in the 19th century. The module covers Genesis–Deuteronomy, Psalms, Isaiah–Malachi, Matthew–1 John and Jude (48 books): Calvin's Joshua is not in it, and he wrote on no other book. |
| `easton` | M. G. Easton, *Illustrated Bible Dictionary* (3rd edition, 1897) | `Easton` 2.0.1 | CCEL | Easton died in 1900. |
| `nave` | Orville J. Nave, *Nave's Topical Bible* (1897) | `Nave` 3.0 | CCEL (`ccel.org/ccel/n/nave/bible.xml`) | Nave died in 1917. |
| `westminster` | Westminster Confession of Faith (1646), Shorter and Larger Catechisms (1647), with their proof texts | `Westminster` 1.0.1 | reformed.org | 17th-century texts of the Westminster Assembly. |
| `lbcf` | Baptist Confession of Faith (1689), with its proof texts | `BaptistConfession1689` 1.0.2 | reformed.org, apuritansmind.com | A 17th-century text. |

Changes: the modules' markup (OSIS, ThML, TEI) becomes plain text with paragraph breaks; a
commentary's footnotes are dropped and a confession's proof-text quotations keep only their
references; a module's title on an entry becomes its heading; references are mapped onto the KJV
verse grid (a Psalm title joins verse 1). The words are not changed. Easton's module tags a
continuation reference ("Rev. 1:8, 11") with the wrong book in places; the text shows the right
one, and the citations are left as the module has them.

### Considered and not yet included

* **The Apostles', Nicene and Athanasian Creeds, the Heidelberg Catechism, the Belgic Confession
  and the Canons of Dort.** CrossWire has no public-domain module for them (its `Westminster21`
  is a copyrighted modern edition). Schaff's *Creeds of Christendom* vol. 3 (1877) on CCEL
  (`https://ccel.org/ccel/schaff/creeds3.xml`, 6,107,301 bytes, SHA-256
  `756adf1ddd466d237221c76b99d25d55cab7e83d08914f28a289c3b90dccddf3`) holds the Heidelberg
  (`div2 iv.vi`), Belgic (`iv.viii`) and Dort (`iv.xvi`) texts, but Heidelberg and Belgic are
  two-column German/French-and-English tables with page breaks inside answers, and Dort's Latin
  precedes its English, so cutting them cleanly is its own job; the Creeds would be in vol. 2,
  which was not checked. archive.org refused the download. Left for a later load.
* **Matthew Henry's Concise Commentary (`MHCC`)** — an abridgement of `mhc`, which is in.

## Fonts — SIL Open Font License 1.1

| Font | Use | Source | Licence |
|---|---|---|---|
| Literata (variable, roman + italic) | Scripture text | github.com/google/fonts `ofl/literata` | [OFL-Literata.txt](../apps/desktop/assets/fonts/OFL-Literata.txt) |
| Inter (variable) | Interface | github.com/google/fonts `ofl/inter` | [OFL-Inter.txt](../apps/desktop/assets/fonts/OFL-Inter.txt) |
| Inter Medium, Inter SemiBold (static instances of the variable Inter, made with `fonttools varLib.instancer`, wght 500/600, opsz 14) | Interface, verse numbers | derived from the above | [OFL-Inter.txt](../apps/desktop/assets/fonts/OFL-Inter.txt) |

The iOS app bundles the same Literata and Inter files, with their licences, in
`apps/ios/Linger/Fonts`.

egui's default fonts (Ubuntu-Light, Hack, Noto Emoji, emoji-icon-font; OFL/Apache/MIT) remain as
fallbacks for symbols.

## Test fixtures

Every importer and online-provider test uses public-domain wording (mostly KJV) in the *format*
of the source being tested (OSIS, Zefania, SWORD, ESV API text, API.Bible JSON). No copyrighted
Bible text appears anywhere in this repository.
