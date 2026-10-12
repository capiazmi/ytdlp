// Rendered by src/pdf.rs. All text comes from data.json, never spliced into markup.
#let d = json("data.json")

#set document(title: d.title)
#set page(
  paper: "a4",
  margin: (x: 2cm, top: 2cm, bottom: 2.2cm),
  footer: context {
    set text(8pt, fill: luma(130))
    d.source
    h(1fr)
    counter(page).display("1 / 1", both: true)
  },
)
#set text(
  font: ("Noto Sans", ..d.cjk, "Noto Sans Arabic", "Noto Sans Hebrew", "Noto Sans Devanagari", "Noto Sans Thai"),
  size: 10.5pt,
  lang: d.lang,
  ..if d.region != none { (region: d.region) },
)
#set par(leading: 0.75em, spacing: 1.1em)

#let accent = rgb("#0b8f9a")

#text(17pt, weight: "bold", d.title)
#v(-4pt)
#text(9.5pt, fill: luma(90), (d.uploader, d.duration).filter(x => x != none).join("  ·  "))

#block(
  width: 100%,
  inset: (x: 10pt, y: 8pt),
  radius: 4pt,
  fill: luma(245),
  text(8.5pt, grid(
    columns: (auto, 1fr),
    column-gutter: 10pt,
    row-gutter: 5pt,
    text(fill: luma(110))[Video], link(d.url, text(fill: accent, d.url)),
    text(fill: luma(110))[Language], [#d.lang_name (#d.code)],
    text(fill: luma(110))[Captions], d.kind,
    text(fill: luma(110))[Exported], datetime.today().display("[year]-[month]-[day]"),
  )),
)
#v(6pt)

#if d.timestamps {
  set text(number-width: "tabular")
  for seg in d.segments {
    let ts = text(8.5pt, fill: accent, weight: "medium", seg.t)
    block(spacing: 0.65em, grid(
      columns: (3.2em, 1fr),
      column-gutter: 8pt,
      if seg.link != none { link(seg.link, ts) } else { ts },
      seg.text,
    ))
  }
} else {
  for p in d.paragraphs { par(p) }
}
