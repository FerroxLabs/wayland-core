---
name: refactor-imports
description: Reorder Rust import groups
when_to_use: After editing imports
---

# Refactor Imports

## Preconditions
- A Rust file is open
- The file compiles
- Imports follow the std → external → internal pattern

## Steps
- Read the file
- Identify the three import groups
- Sort each group alphabetically
- Group blank lines as needed
- Re-run cargo check
