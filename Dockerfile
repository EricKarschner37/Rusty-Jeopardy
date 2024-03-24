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
RUN apk add --update --no-cache python3 py3-pip py3-virtualenv
COPY requirements.txt .
RUN python3 -m venv /opt/venv && source /opt/venv/bin/activate
ENV PATH="/opt/venv/bin:$PATH"
RUN pip3 install -Ur requirements.txt

EXPOSE 10001

CMD ["jeopardy"]
