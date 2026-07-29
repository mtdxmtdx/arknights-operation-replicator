# Start from an AFA-paused battle and take over after a user focus handoff

The Start action now assumes the user has already entered the stage and AFA has paused it. The runner never focuses the game automatically: it waits for the user to click the game, requires a newer trustworthy ruler sample after that handoff, performs formation binding, then repeats the handoff and uses the next trustworthy 1x sample as the Machine takeover sample. This removes the old race between a pre-start arm loop and AFA's automatic opening pause while keeping manual binding safe by allowing it only when the first post-focus sample is paused.

The Start button is enabled only when a job is loaded, AFA is ready, and the current ruler sample is a trustworthy in-battle `1x_paused` sample no later than the first action. A running first-focus sample therefore permits only a complete saved binding profile; it never opens the binding panel or sends an AFA pause key.
