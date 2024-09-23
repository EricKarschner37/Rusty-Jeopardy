#!/opt/venv/bin/python
from bs4 import BeautifulSoup
from urllib.request import urlopen
import csv
import sys
import os

def get_from_url(url):
    if not os.environ.get('J_GAME_ROOT'):
        print("Error: `J_GAME_ROOT` environment variable should be set. See documentation for details.")
        sys.exit(1)
    root = f'{os.environ.get("J_GAME_ROOT")}/{url[-4:]}'
    if os.path.exists(root):
        print(f"Error: {J_GAME_ROOT}/{url[-4:]} already exists.")
        sys.exit(2)

    html = urlopen(url).read()
    html = html.replace(b"&lt;", b"<")
    html = html.replace(b"&gt;", b">")
    soup = BeautifulSoup(html)
    table = soup.select_one('div#jeopardy_round').select_one('table.round')

    single_cat, single_clues, single_answers = pull_from_table(table)
    table = soup.select_one('div#double_jeopardy_round').select_one('table.round')
    double_cat, double_clues, double_answers = pull_from_table(table)
    final_table = soup.select_one("table.final_round")
    final_cat = final_table.select_one("td.category_name").text
    final_clue = soup.select_one("td#clue_FJ").text
    final_str = str(final_table)
    final_answer = clean_str(final_str[final_str.index('correct_response')+16:final_str.index('/em')])

    os.mkdir(root)

def pull_default_from_table(table, name, round_multiplier=2):
    categories = [{'category': td.text, 'clues': []} for td in table.select("td.category_name")]

    clue_rows = table.find_all("tr", recursive=False)
    for row_i, tr in enumerate(clue_rows):
        clueEls = tr.select("td.clue")
        for i, td in enumerate(clueEls):
            cost = 100 * (row_i + 1) * round_multiplier
            clue = td.select_one("td.clue_text").text or "This clue was missing"
            response = td.select_one("em.correct_response").text or "This response was missing"
            is_daily_double = td.select_one("td.clue_value_daily_double") is not None
            categories[i]['clues'].append({'clue': clue, 'response': response, 'cost': cost, 'is_daily_double': is_daily_double})

def pull_from_table(table):
    categories = [td.text for td in table.select("td.category_name")]
    double = False

    clues = [[("Daily Double: " if 'clue_value_daily_double' in str(td) else "") + td.select_one("td.clue_text").text if td.find('td', {'class': 'clue_text'}) else 'This clue was missing' for td in tr.select("td.clue") ] for tr in table.select("tr") if tr.find("td", {'class': 'clue'})]
    answers = [[clean_str(str(td)[str(td).index('correct_response')+16:str(td).index('/em')]) if 'correct_response' in str(td) else 'This answer was missing' for td in tr.select("td.clue")] for tr in table.select("tr") if tr.find("td", {'class': 'clue'})]
    return categories, clues, answers

if __name__ == '__main__':
    get_from_url(f"https://www.j-archive.com/showgame.php?game_id={sys.argv[1]}")
