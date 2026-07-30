# Keep editing as a zero-input application mode

The job editor lives in the existing `repl-app` window, but Edit and Replicate are mutually exclusive modes. While Edit is active the application may read and write job files, load logical map data, and read ruler snapshots. It must not probe AFA, locate the game window, construct `Session` or `Runner`, or call any input primitive. Starting a run is rejected while Edit is active, and a live run prevents switching back to Edit.

This boundary lets operators author jobs offline and makes “follow ruler” a read-only convenience instead of an automation shortcut. Automatic mouse recording or deploy recognition is deliberately outside the first editor release.
