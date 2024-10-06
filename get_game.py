#!/opt/venv/bin/python
from bs4 import BeautifulSoup
from urllib.request import urlopen
import csv
import sys
import os
import json
import re

def get_game_with_id(game_id):
    url = f"https://www.j-archive.com/showgame.php?game_id={game_id}"
    html = urlopen(url).read()
    html = html.replace(b"&lt;", b"<")
    html = html.replace(b"&gt;", b">")
    soup = BeautifulSoup(html)
    table = soup.select_one('div#jeopardy_round').select_one('table.round')

    rounds = []

    rounds_unparsed = soup.find_all('div', id=re.compile('.*jeopardy_round'))
    for r in rounds_unparsed:
        default_round = r.select_one('table.round')
        title = r.select_one('h2').text
        if default_round:
            smallest_clue_value = int(r.select_one('td.clue_value').text[1:])
            rounds.append(pull_default_from_table(default_round, name=title, round_multiplier=smallest_clue_value // 100, default_max_wager=smallest_clue_value*100))
        final_round = r.select_one('table.final_round')
        if final_round:
            rounds.append(pull_final_from_table(final_round, name=title, default_max_wager=3000))

    return {'rounds': rounds}


def pull_default_from_table(table, name, round_multiplier, default_max_wager):
    categories = [{'category': td.text, 'clues': []} for td in table.select("td.category_name")]

    clue_rows = table.find_all("tr", recursive=False)[1:] # The first row is categories
    for row_i, tr in enumerate(clue_rows):
        clueEls = tr.select("td.clue")
        for i, td in enumerate(clueEls):
            cost = 100 * (row_i + 1) * round_multiplier
            clue_td = td.select_one("td.clue_text")
            clue = clue_td.text if clue_td else "This clue was missing"
            response_td = td.select_one('em.correct_response')
            response = response_td.text if response_td else "This response was missing"
            is_daily_double = td.select_one("td.clue_value_daily_double") is not None
            categories[i]['clues'].append({'clue': clue, 'response': response, 'cost': cost, 'is_daily_double': is_daily_double})
    return {'categories': categories, 'name': name, 'round_type': 'DefaultRound', 'default_max_wager': default_max_wager}

def pull_final_from_table(table, name, default_max_wager):
    category = table.select_one('td.category_name').text
    clue = table.select_one("td#clue_FJ").text
    response = table.select_one('em.correct_response').text
    return {'category': category, 'clue': clue, 'response': response, 'round_type': 'FinalRound', 'name': 'Final Jeopardy', 'default_max_wager': default_max_wager}


if __name__ == '__main__':
    games_root = os.environ.get('J_GAME_ROOT') or 'games'
    game_id = sys.argv[1]
    payload = get_game_with_id(game_id)
    out_filename = f'{games_root}/{game_id}.json'
    with open(out_filename, 'w') as out_file:
        json.dump(payload, out_file)
