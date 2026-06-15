Output assets/cx-demo.gif

Require bash

Set Shell "bash"
Set FontSize 14
Set FontFamily "Monaco"
Set Width 1600
Set Height 760
Set LetterSpacing 0
Set LineHeight 1.1
Set Padding 18
Set Framerate 24
Set TypingSpeed 22ms
Set WindowBar Colorful
Set Theme "Catppuccin Mocha"

Hide
Type "export HOME=/tmp/cx-demo/home" Enter
Type "export CX_HOME=$HOME/.cx" Enter
Type "export CX_CODEX_BIN=/tmp/cx-demo/bin/codex" Enter
Type "export PATH=/tmp/cx-demo/bin:$PATH" Enter
Type "export PS1='$ '" Enter
Type "clear" Enter
Show

Type "cx watch --once" Enter
Sleep 2s

Type "cx sessions --limit 4" Enter
Sleep 2s

Type "cx resume-here --smart" Enter
Sleep 3s

Type "cx doctor" Enter
Sleep 4s
