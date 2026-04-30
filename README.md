## Towards a Mouse Free UX for Most APPs on macOS (WIP)

This tiny tool aims to ease the pain of

1. Mouse selection (mainly text)
2. Selecting actions in the pop-up menu after right clicking
3. Recurring tasks given selected text

- You can think of it as a purely keyboard and open-source version of
[PopClip](https://www.popclip.app/) with some extra utility features.
- And it allows you to interact with some UI text that is not
even possible to select using a mouse.
Like those in a video game or in a button.

## Demo

### Text Manipulation

Text can be extracted from either UI elements or Apple VisionKit OCR results

<https://github.com/user-attachments/assets/9a75dd87-a61a-4e8c-9bee-41dcb59c285f>

### Image Copying and Input Text Editing

For users who are not satisfied with the default text editing experiences of editable
text fields, this app allows you to edit them in your favorite editor,
and automatically sync the saved content back to the UI element.

<https://github.com/user-attachments/assets/2ecb1a80-435c-467d-abfa-e58bde521a00>

### Multi-selection

1. Toggle multi-selection mode on (Shift key)
2. Select starting/ending piece of text
3. Select the other side, and the program will automatically guess the
paragraph of intention

Here's an example of how to select and translate the lyrics in Apple Music.

<https://github.com/user-attachments/assets/e1a2d66d-627a-4bd4-8601-90b841fb477e>

#### Precise selection

If you want to select a specific piece of text in an identified element, you can

1. Split the whole context into pieces, the interface of word picker will pop up
2. Toggle multi-selection mode on within the word picker
3. Select both sides according to the hint keys

<img width="440" height="387" alt="Image" src="https://github.com/user-attachments/assets/7320969c-344f-40bd-b74d-960768420b2e" />

### Other Features

- UI element tree exploring mode (E)
  - Useful for debugging and screenshot taking
- Apple Dictionary support in simple pop-up window
  - Avoids the hassle of opening the dictionary app, like what PopClip will do
  - Dictionary CSS is respected to make the text more readable

<img width="363" height="316" alt="Image" src="https://github.com/user-attachments/assets/5d89c973-c043-4ba9-a760-727e81fc5c96" />

- Easily extensible text actions, please refer to the [Configuration](#configuration) section
  - Avoids the hassle of plugin management
- Act on text from clipboard
- Customizable theme

## Installation

At this pre-alpha stage, you can try this app by:

1. Download the latest version from the [releases page](https://github.com/blindFS/Glyphlow/releases).
2. Extract it and strip the quarantine information: `xattr -c glyphlow`
3. Run `glyphlow` in a terminal, and grant the accessibility permission
to your terminal app when prompted at the first time.

## Purging

This app is designed to be lean and clean, it only generates 2 files:

1. A configuration file `$XDG_CONFIG_HOME/glyphlow/config.toml` or
`~/.config/glyphlow/config.toml` if the env-var is not set.
2. A cache file for temporary editing: `$XDG_CACHE_HOME/glyphlow/tempfile.md`
or `~/.cache/glyphlow/tempfile.md`.

## Configuration

Here's how I configure it to perform those actions shown in the demo videos.
A comprehensive configuration file is generated when you run this app at the first time.

```toml
colored_frame_min_size = 100
element_min_width = 15
element_min_height = 15
ocr_languages = [
  "zh-Hans",
  "ja-JP",
  "en-US",
]
dictionaries = [
  "牛津英汉汉英词典",
  "New Oxford American Dictionary",
]

[[text_actions]]
display = "󰊭 Google Search"
key = 'G'
command = "nu"
args = ["-c", "r#'{glyphlow_text}'# | url encode | ^open $'https://google.com/search?q=($in)'"]

[[text_actions]]
display = "󰖬 Wikipedia Search"
key = 'W'
command = "nu"
args = ["-c", "r#'{glyphlow_text}'# | url encode | ^open $'https://en.wikipedia.org/wiki/Special:Search/($in)'"]

[[text_actions]]
display = "󰊿 Goolge Translate -> zh_cn"
key = 'T'
command = "nu"
args = ["-c", "r#'{glyphlow_text}'# | url encode | ^open $'https://translate.google.com/?sl=auto&tl=zh_cn&text=($in)&op=translate'"]

[editor]
display = " Editor"
key = 'V'
command = "tmux"
args = ["new-window", "-t", "dev", "^open -a Ghostty; ^nvim {glyphlow_temp_file}"]

[theme]
hint_font = "AndaleMono:16"
menu_font = "IosevkaTerm Nerd Font Mono:26"
```

## Roadmap

1. [ ] nix-flake/homebrew-tap, and make it a user service
2. [ ] menu bar icon
