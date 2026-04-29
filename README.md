## Towards a Mouse Free UX for Most APPs on macOS (WIP)

## Demo

### Text Manipulation

Text extracted from UI elements or Apple VisionKit OCR results

<https://github.com/user-attachments/assets/9a75dd87-a61a-4e8c-9bee-41dcb59c285f>

### Image Copying and Prompt Editing

<https://github.com/user-attachments/assets/2ecb1a80-435c-467d-abfa-e58bde521a00>

### Multi-selection

1. Toggle multi-selection mode on
2. Select starting/ending piece of text
3. Select the other side, and the program will automatically guess the paragraph of intention

<https://github.com/user-attachments/assets/e1a2d66d-627a-4bd4-8601-90b841fb477e>

## Installation

At this pre-alpha stage, you can try this app by:

1. Download the latest version from the [releases page](https://github.com/blindFS/Glyphlow/releases).
2. Extract it and strip the quarantine information: `xattr -c glyphlow`
3. Run `glyphlow` in a terminal, and grant the accessibility permission to your terminal app when prompted at the first time.

## Purging

This app is designed to be lean and clean, it only generates 2 files:

1. A configuration file `$XDG_CONFIG_HOME/glyphlow/config.toml` or `~/.config/glyphlow/config.toml` if the env-var is not set.
2. A cache file for temporary editing: `$XDG_CACHE_HOME/glyflow/tempfile.md` or `~/.cache/glyphlow/tempfile.md`.

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
