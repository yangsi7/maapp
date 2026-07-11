# maapp init demo — dry-run project wiring in a scratch dir
# Render from the repo root: vhs demo/init.tape

Output demo/init.gif

Set Shell "bash"
Set FontSize 16
Set Width 1200
Set Height 420
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

Hide
Type "mkdir -p /tmp/maapp-init-demo/.claude && cd /tmp/maapp-init-demo && clear"
Enter
Sleep 1s
Show

Type "maapp init --dry-run"
Sleep 500ms
Enter
Sleep 4s
