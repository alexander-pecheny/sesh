A large language model is a program that predicts the next piece of text, one piece at a time. It learns those predictions by reading a huge amount of written text.

## 1. The core task: guess the next word

Suppose you see "The cat sat on the ___". You would probably guess "mat" or "floor". An LLM does the same thing at a much larger scale. Given some text, it produces a probability for every possible next piece, picks one, adds it to the text, and asks again.

## 2. Tokens: the pieces of text

Models do not read whole words or single letters. They read tokens, which are chunks of text that usually fall somewhere between the two. Common words such as "the" are usually one token each.

- A typical vocabulary holds 50,000 to 200,000 tokens.
- Tokens explain some odd failures, such as miscounting the r's in "strawberry".
- Each token has an ID number, and the model sees only those numbers.

## 3. Turning tokens into numbers with meaning

A bare ID number says nothing about meaning. So each token is mapped to an *embedding*, which is a long list of numbers. You can think of an embedding as a point in a space with thousands of dimensions. During training, tokens with related meanings end up near each other in this space.

## 4. The neural network

The model itself is a very large mathematical function with adjustable numbers called parameters or weights. Each layer takes lists of numbers, multiplies them by its weights, adds them up, applies a simple non-linear step, and passes the result on.

| Stage | What happens |
|---|---|
| Tokenizing | Text is split into tokens and turned into IDs |
| Embedding | Each token ID becomes a vector |
| Attention | Tokens look at each other to gather context |
| Output | The model gives a probability for every next token |

## 5. Training

A new model starts with random weights and produces gibberish. In pre-training, the model reads a vast body of text. At each position it predicts the next token, then checks the real one, and nudges every weight a little in the direction that would have made the right answer more likely.
