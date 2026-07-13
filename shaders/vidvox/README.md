# Vendored Vidvox ISF filters

A curated subset of the [Vidvox ISF-Files](https://github.com/Vidvox/ISF-Files)
collection by Vidvox, MIT licensed — the upstream license text is vendored
verbatim in [LICENSE](LICENSE) as the MIT terms require. Chosen for the effect
families VJs reach for most: glitch, kaleidoscope/mirror, halftone, distortion,
strobe, and feedback trails. Only single-`.fs` filters are vendored — files
with a paired `.vs` vertex shader need custom vertex stages the vidiotic ISF
transpiler doesn't run.

Every file here is compile-tested against the transpiler by
`bundled_isf_shaders_compile` (src/shader.rs); a file that stops transpiling
should be fixed or dropped, not skipped.

Load one onto a cue with the chain editor's ISF picker, same as any other
`.fs`.
