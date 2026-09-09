# Credits

## Hedronite

Lapis is a Hedronite product. Spec, schemas, and operator canon live in the Atrium vault (`foundry/lapis/`).

## ZenNotes / `zn`

Lapis is **not a fork** of [ZenNotes/tui](https://github.com/ZenNotes/tui). We copy selected MIT-licensed components (Vim engine, TUI chrome, task-line grammar, periodic notes, template substitution) because that top layer is what we wanted, and we would have deleted most of a fork.

When those files land here, they keep their notices. The ZenNotes license, quoted in full:

```
MIT License

Copyright (c) 2026 Adib Hanna and ZenNotes contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Reference clone on castle (steal-source, not this repo): `~/Developer/zennotes-tui` @ `e5550a380eebb09e4cebf258761b47cb394789cc`.

## Atrium Lattice

Search, document metadata, and the wikilink graph are owned by Atrium Lattice (`projects/atrium-lattice/` in the vault). Lapis is a client. It does not reimplement `is_walked` and it does not write `lattice.db`.
