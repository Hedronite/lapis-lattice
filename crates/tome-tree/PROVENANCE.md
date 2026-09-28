# Provenance

`tome-tree` is a clean-room implementation.

Concepts taken from [VectifyAI/PageIndex](https://github.com/VectifyAI/PageIndex) at commit `619cbd8` (MIT):

- a table-of-contents tree whose nodes span pages
- the model walks that tree and chooses which nodes to open, with no vector step for "which section"
- nodes that run too long are split

No PageIndex source was copied, translated line by line, or kept open while this crate was written. Prompts, JSON field names, and the file layout are ours. The license notice below records the inspiration only.

```
MIT License

Copyright (c) 2025 Vectify AI

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
