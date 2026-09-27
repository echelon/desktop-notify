# Agent artwork

`codex.svg` uses OpenAI's six-loop knot to identify Codex. The path is the bold
small-size variant distributed with OpenAI's desktop app (bundle ID
`com.openai.codex`), copied from
`webview/assets/openai-logo-bold-bounding-box-b5f4839d5bdf.svg` inside its
`Contents/Resources/app.asar` on 2026-09-27. The path is unchanged; the fill is
light for the notifier's dark background and the artboard padding is reduced.

The notifier uses this artwork only to identify rows reported by Codex. Keep
the asset local so agent icons never require network access. The image is
displayed at 17 CSS pixels. Vector geometry keeps the interlocking loops sharp
on Retina displays. OpenAI retains ownership of its artwork and marks.
