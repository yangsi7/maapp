# maapp hero demo — validate, blast-radius, render spine
# Render from the repo root: vhs demo/demo.tape

Output demo/demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 1200
Set Height 650
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

Type "maapp validate examples/checkout.json"
Sleep 500ms
Enter
Sleep 2.5s

Type "maapp query blast-radius store:CheckoutStore examples/checkout.json"
Sleep 500ms
Enter
Sleep 3.5s

Type "maapp render spine examples/checkout.json"
Sleep 500ms
Enter
Sleep 5s
