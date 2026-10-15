# pumice-formatter

Formatter crate for Pumice: injects an explicitly selected formatter's
rules, as one `system` message, into an OpenAI-shaped chat request before
it reaches the relay. Pure text transformation — no provider calls, no
model rewrites, no network — and it never logs request text.

The library powers both the standalone `pumice-format` binary and the
in-process `pumice format …` subcommand of the orchestrator
(arch-modules/D13).

## Building and testing

```bash
cargo test -p pumice-formatter
```

There is no native code and no feature flag: the default build is the full
CLI.

## Usage

```bash
pumice-format list
pumice-format show builtin:clean > my.yaml
echo '{"model":"claude","messages":[{"role":"user","content":"reunião às nove horas"}]}' \
  | pumice-format apply --formatter builtin:clean
pumice-format check-config
```

Formatters are one YAML file each (`schema_version`, `id`, `description`,
`system`) registered in `formatter.yml`. See
[`docs/formatter.md`](../../docs/formatter.md) for the definition format,
the conflict policy (`reject` by default, `append` keeps existing
instructions), `--json` envelopes and exit codes.

## Third-party licenses

The built-in `builtin:clean` formatter is adapted from the default
transcript-cleanup prompt of
[Handy](https://github.com/cjpais/Handy) by CJ Pais, used under the
[MIT license](https://github.com/cjpais/Handy/blob/main/LICENSE)
(`Copyright (c) 2025 CJ Pais`). The full license text is in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) in this crate, which ships
with every release archive and npm package, and is repeated as YAML comments
in the output of `show builtin:clean`. The adaptation rewords the prompt as a
system message for dictation that arrives as the user message and drops the
`<transcript>` envelope wording (p11-formatter/D1).
