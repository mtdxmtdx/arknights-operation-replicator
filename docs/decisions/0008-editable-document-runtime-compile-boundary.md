# Separate editable JSON documents from runnable jobs

`JobDocument` owns the original JSON object, stable in-memory action IDs, editing operations, and diagnostics. It deliberately permits incomplete or malformed known fields and preserves unknown fields when known fields are edited. Such drafts may be saved.

`Copilot` remains the strict runtime model. The UI compiles the current `JobDocument` through the existing `Copilot::parse` boundary after edits. Only a successful compile with no error diagnostics replaces the runnable job; otherwise the previous runnable value is cleared and Start remains disabled. Warnings, including an early first action, do not prevent compilation.

This avoids weakening runtime invariants merely to support incremental editing and provides a single explicit handoff from authoring to automation.
