# Semantic LSP smoke fixtures

These minimal fixtures verify the project-scoped semantic MCP bridge for the
language servers currently installed on this host:

- C through clangd
- Rust through rust-analyzer
- Python through BasedPyright
- JavaScript and TypeScript through typescript-language-server
- Dart through the Dart SDK language server

They are tooling probes, not an application architecture decision.

Generate the local C compilation database before checking clangd:

```sh
cmake -S tools/lsp-smoke/c -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
```

The generated `build/` directory is intentionally ignored by Git.
