# Tokenizer vocabulary

`o200k_base.tiktoken` is OpenAI's o200k vocabulary, the one GPT-4o and later use: one token per
line, its bytes in base64, then its rank. It comes unchanged from OpenAI's
[tiktoken](https://github.com/openai/tiktoken) (MIT licence), by way of the
[tiktoken-rs](https://github.com/zurawiki/tiktoken-rs) crate (MIT licence).

`src/core/counts.rs` builds its token counter from it.
