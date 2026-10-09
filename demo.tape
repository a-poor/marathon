# Regenerate with the local build; see README.md for the command.
Output assets/demo.gif
Require marathon

Set Shell "bash"
Set FontSize 18
Set Width 1200
Set Height 900
Set Padding 20
Set TypingSpeed 65ms
Set Theme "Catppuccin Mocha"

Env NO_COLOR ""
Env COLORTERM "truecolor"

Type "marathon run samples/demo.md"
Enter
Wait+Screen /Hello, Marathon/
Sleep 2s

# Run remaining: greet the world, then pause for an answer.
Type "r"
Wait+Screen /Who are you/
Sleep 1s
Down
Sleep 700ms
Enter
Wait+Screen /Hello, Bob!/
Wait+Screen /exit 1/
Sleep 2s

# Search the document; cycle to the next match and back.
Type "/step"
Sleep 1s
Enter
Sleep 1s
Type "n"
Sleep 1s
Type "N"
Sleep 1s
Escape

# Run the selected loop and return to its output.
Enter
Sleep 500ms
Up
Sleep 3s

# Quit cleanly so the demo leaves no active commands or scratch directories.
Hide
Type "q"
Sleep 500ms
