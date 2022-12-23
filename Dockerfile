FROM rust:latest as builder
WORKDIR /usr/src/jeopardy
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY src ./src
RUN cargo install --path .

FROM alpine:latest
COPY --from=builder /usr/local/cargo/bin/jeopardy /usr/local/bin/jeopardy
COPY get_game.py /usr/local/bin/

RUN apk add --update --no-cache gcompat

RUN mkdir ./games
ENV JEOPARDY_GAME_ROOT=games/
ENV J_GAME_ROOT=games
RUN chmod ugo+w games -R

ENV PYTHONUNBUFFERED=1
RUN apk add --update --no-cache python3 && ln -sf python3 /usr/bin/python
RUN python3 -m ensurepip
RUN pip3 install --no-cache --upgrade pip setuptools
COPY requirements.txt .
RUN python3 -m pip install -r requirements.txt

EXPOSE 10001

CMD ["jeopardy"]
