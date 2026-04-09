# Tree-Sitter Imports Demo

Simple TS project used to exercise `devsql`'s tree-sitter-backed `imports` table.

```
cd /Users/douglance/Developer/devsql
cargo run --bin devsql -- -r examples/import-demo "SELECT file_path, line_number, module, name, alias, kind FROM imports"
```
