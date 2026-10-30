# BTN-00/01 terminal cell prototype

Synthetic cell layouts for the shared filled-control geometry (label plus one
cell on each side). Brackets mark the button boundary; `·` is a visible blank.
The status row records the semantic role/state shown by style tokens, not host
theme detection.

## Normal width, 22 cells

```text
Pane header: [Menu]·>Pane 1·······[x]
Menu role: secondary / focus: pane focus / close role: destructive
```

## Minimum pane width, 20 content cells plus existing two-cell horizontal padding

```text
Pane header: [Menu]·>···········[x]
Label cells: 0 (truncated before controls); hitboxes do not overlap.
```

## Semantic control samples

```text
normal       [ Apply ]
focus        [ Apply ]  marker: >
pressed      [ Apply ]  marker: *
disabled     [ Apply ]  marker: -
pending      [ Apply ]  marker: ~
```

Color-role mapping: secondary uses neutral contrast, primary uses the action
accent, destructive uses danger contrast. Reduced-color mode uses default
colors and the listed structural markers. Light/dark roles are semantic palette
tokens; this synthetic fixture does not assert a specific host palette or
terminal theme response.
