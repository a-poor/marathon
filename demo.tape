# Regenerate with the local build; see README.md for the command.
Output assets/demo.gif
Require marathon
Require curl

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
Sleep 3s

# Fetch sample JSON with curl, then pause for a name.
Type "r"
Wait+Screen /submit/
Sleep 2500ms
Down
Sleep 800ms
Enter

# The chosen name feeds the next command. Hold on the successful result.
Wait+Screen /Hello, Bob!/
Sleep 5s

# Quit off-camera to clean up the session.
Hide
Type "q"
Sleep 500ms
