#set document(title: [hex manual], date: none)
#set page(paper: "a4", fill: white, margin: 19mm, footer: context [
  #set text(size: 8pt, fill: rgb("68747a"))
  #grid(columns: (1fr, auto), [hex · $version$], counter(page).display("1"))
])
#set text(font: "Ioskeley Mono", size: 9pt, fill: rgb("252a2d"), lang: "en")
#show raw: set text(font: "Ioskeley Mono")
#set par(leading: .7em, spacing: .8em)
#set heading(numbering: none)
#set table(stroke: .4pt + rgb("c8cece"), inset: 6pt)
#show link: set text(fill: rgb("2a596b"))
#show heading.where(level: 1): set text(size: 18pt, fill: rgb("263e48"))
#show heading.where(level: 2): set text(size: 12pt)
#text(size: 28pt, weight: "bold")[hex manual]
#parbreak()
#text(size: 10pt)[$subtitle$]
#v(14pt)
#outline(title: [contents], depth: 1)
#pagebreak()
$body$
