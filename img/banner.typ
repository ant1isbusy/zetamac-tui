#let background = rgb("#1d2021")
#let foreground = background.lighten(80%)

#let lcg-next(state) = {
  let a = 1664525
  let c = 1013904223
  let m = 4294967296 // 2^32
  let next-state = calc.rem(a * state + c, m)
  (next-state, next-state / m)
}

#let lcg-int(state, low, high) = {
  let (next-state, val) = lcg-next(state)
  let result = low + calc.floor(val * (high - low + 1))
  (next-state, result)
}

#set page(
  width: 1073pt,
  height: 151pt,
  margin: 0em,
  fill: none,
  background: {
    let seed = 1337
    let cols = 14
    let rows = 4
    let items = ()

    for _ in range(rows * cols) {
      let a = 0
      let b = 0
      (seed, a) = lcg-int(seed, 1, 20)
      (seed, b) = lcg-int(seed, 1, 20)
      items.push([#a×#b])
    }

    place(top + left)[
      #box(
        width: 1073pt,
        height: 151pt,
        fill: background,
        radius: 15pt,
        clip: true,
      )[
        #place(center + horizon)[
          #grid(
            columns: (72pt,) * cols,
            rows: (26pt,) * rows,
            align: center + horizon,
            ..items.map(it => text(
              size: 16pt,
              font: "Monaspace Krypton NF",
              fill: background.lighten(12%),
              it,
            ))
          )
        ]
      ]
    ]
  },
)

#set text(
  size: 70pt,
  fill: foreground,
  spacing: 10pt,
  font: "Monaspace Krypton NF",
)
#set align(center + horizon)

#text(1.9em)[*zetamac*]
tui
