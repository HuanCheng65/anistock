"""collect.py's Filmarks parsers, on markup shaped like the site's (2026-11; no copied page content)."""

from datetime import date

import collect

CARD = ('<div class="js-cassette" data-clip="{&quot;anime_series_id&quot;:10,&quot;anime_season_id&quot;:20,'
        '&quot;count&quot;:5380}" data-mark="{&quot;anime_series_id&quot;:10,&quot;anime_season_id&quot;:20,'
        '&quot;count&quot;:2723}"><h3 class="p-content-cassette__title">作品 A</h3>'
        '<h4 class="p-content-cassette__other-info-title">公開日：</h4><span>2026年10月4日</span>'
        '<div class="c-rating__score">4.0</div></div>')


def test_a_season_list_card():
    unrated = CARD.replace("4.0", "-").replace(":20", ":21")
    works = collect.parse_filmarks_list("<html>" + CARD + unrated)
    assert works[0] == {"series_id": 10, "season_id": 20, "title": "作品 A", "released": "2026-10-04",
                        "marks": 2723, "clips": 5380, "score": 4.0}
    assert works[1]["season_id"] == 21 and works[1]["score"] is None


def test_a_work_page():
    page = ('<script type="application/ld+json">{"@type":"TVSeason","startDate":"2026-10-02T00:00:00+09:00",'
            '"aggregateRating":{"ratingValue":"4.2","reviewCount":"56"}}</script>'
            '<li class="p-content-detail-links__item p-content-detail-links__item--official"><a target="_blank" '
            'href="https://example.jp/s3/">公式サイト</a></li>'
            '<div class="c2-episode-list-item__header-text-title">Prime Video</div>'   # a streaming list, not an episode
            '<div class="c2-episode-list-item"><div class="c2-episode-list-item__header-text-title">第1話</div>'
            '<a class="c2-episode-list-item__outline-link" href="/animes/1/2/episodes/300"><p>コメント1,085件</p><p>拍手135回</p></a>')
    w = collect.parse_filmarks_work(page)
    assert w["episodes"] == [{"episode_id": 300, "comments": 1085, "claps": 135, "title": "第1話"}]
    assert (w["rating"], w["reviews"], w["official"]) == ("4.2", "56", "https://example.jp/s3/")


def test_the_current_and_next_season():
    assert collect.filmarks_seasons(date(2026, 11, 9)) == [(2026, 7), (2026, 10), (2027, 1)]
    assert collect.filmarks_seasons(date(2027, 3, 31)) == [(2026, 10), (2027, 1), (2027, 4)]
