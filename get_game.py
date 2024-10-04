#!/opt/venv/bin/python
from bs4 import BeautifulSoup
from urllib.request import urlopen
import csv
import sys
import os
import json

def get_game_with_id(game_id):
    url = f"https://www.j-archive.com/showgame.php?game_id={game_id}"
    html = urlopen(url).read()
    html = html.replace(b"&lt;", b"<")
    html = html.replace(b"&gt;", b">")
    soup = BeautifulSoup(html)
    table = soup.select_one('div#jeopardy_round').select_one('table.round')

    single_round_unparsed = soup.select_one('div#jeopardy_round').select_one('table.round')
    single_round = pull_default_from_table(single_round_unparsed, 'Jeopardy')

    double_round_unparsed = soup.select_one('div#double_jeopardy_round').select_one('table.round')
    double_round = pull_default_from_table(double_round_unparsed, 'Double Jeopardy', 4)

    final_round_unparsed = soup.select_one('div#final_jeopardy_round').select_one('table.final_round')
    final_round = pull_final_from_table(final_round_unparsed, 'Final Jeopardy')

    rounds = [single_round, double_round, final_round]

    return {'rounds': rounds}

def get_default_max_wager_for_round(round_name):
    if round_name == 'Jeopardy':
        return 1000
    if round_name == 'Double Jeopardy':
        return 2000
    if round_name == 'Final Jeopardy':
        return 3000
    return 1000


def pull_default_from_table(table, name, round_multiplier=2):
    categories = [{'category': td.text, 'clues': []} for td in table.select("td.category_name")]

    clue_rows = table.find_all("tr", recursive=False)[1:] # The first row is categories
    for row_i, tr in enumerate(clue_rows):
        if row_i == 0:
        clueEls = tr.select("td.clue")
        for i, td in enumerate(clueEls):
            cost = 100 * (row_i + 1) * round_multiplier
            clue = td.select_one("td.clue_text").text or "This clue was missing"
            response = td.select_one("em.correct_response").text or "This response was missing"
            is_daily_double = td.select_one("td.clue_value_daily_double") is not None
            categories[i]['clues'].append({'clue': clue, 'response': response, 'cost': cost, 'is_daily_double': is_daily_double})
    return {'categories': categories, 'name': name, 'round_type': 'DefaultRound', 'default_max_wager': get_default_max_wager_for_round(name)}

def pull_final_from_table(table, name):
    category = table.select_one('td.category_name').text
    clue = table.select_one("td#clue_FJ").text
    response = table.select_one('em.correct_response').text
    return {'category': category, 'clue': clue, 'response': response, 'round_type': 'FinalRound', 'name': 'Final Jeopardy', 'default_max_wager': get_default_max_wager_for_round('Final Jeopardy')}


if __name__ == '__main__':
    games_root = os.environ.get('J_GAME_ROOT') or 'games'
    game_id = sys.argv[1]
    payload = get_game_with_id(game_id)
    out_filename = f'{games_root}/{game_id}.json'
    with open(out_filename, 'w') as out_file:
        json.dump(payload, out_file)
