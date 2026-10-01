"""What the game has, sorted into the categories the import dialogs offer.

Models are sorted by the archive folder the game keeps them in (`Items_*`, `Objects_Nat_01`,
`Levelmesh_*`, …), actors by theirs (`_emfx36/Humans/Bodys`, `…/Heads/…`, `…/Monster/…`), and motion
clips by the words of their names (`…_Attack_…`, `…_Sit…`, `…_Say_…`). Lists are read once per session.
"""

from . import core

ALL = "all"

MESH_CATEGORIES = (
    ("buildings", "Будівлі", "Будинки, інтер'єри, башти — частини рівнів"),
    ("locations", "Локації", "Цілі шматки рівнів: місто, монастир, печери, підземелля"),
    ("items", "Предмети інвентаря", "Зброя, щити, шоломи, їжа, рослини, речі"),
    ("decor", "Декор і меблі", "Меблі, бочки, ящики, килими, двері, інтерактивні об'єкти"),
    ("nature", "Оточення", "Камені, кістки, корені, природні об'єкти"),
    ("trees", "Дерева й кущі", "Дерева, кущі й водорості гри (SpeedTree): заглушка потрібного розміру з корою й листям — для оточення, не для редагування"),
    ("plants", "Трава й чагарник", "Пучки трави, гілки, наземний чагарник (звичайні моделі)"),
    ("water", "Вода", "Море, озера, водоспади"),
    ("technical", "Службові", "Оклюдери, маркери карти, тестові й редакторські об'єкти"),
)

_BUILDING_WORDS = ("_in_", "house", "hut", "tower", "barracks", "church", "tavern", "library", "kitchen",
                   "storehouse", "school", "bath", "entrance", "temple", "chapel", "smithy", "inn", "castle")


def mesh_category(entry):
    folder = entry.strip("/").split("/")[0].lower()
    name = entry.rsplit("/", 1)[-1].lower()
    if folder.startswith("items_"):
        return "items"
    if folder in ("objects_misc_01", "objects_interacts_01", "objects_evt_01"):
        return "decor"
    if folder == "objects_nat_01":
        return "nature"
    if folder == "objects_brushes_01":
        return "plants"
    if folder.startswith("levelmesh_water"):
        return "water"
    if (folder in ("editor_editsupporter", "editsupporter") or folder.startswith("testkram")
            or any(w in folder for w in ("occluder", "weatherzone", "mapmarker", "shadowbox", "waitingroom", "testlevel"))):
        return "technical"
    if folder.startswith("levelmesh"):
        return "buildings" if any(w in name for w in _BUILDING_WORDS) else "locations"
    return "decor"


ACTOR_CATEGORIES = (
    ("body", "Люди: тіло", "Тіла й обладунки людей (голова — окремо)"),
    ("head", "Люди: голова", "Голови людей"),
    ("monster", "Монстри", "Тварини й монстри"),
    ("object", "Анімовані об'єкти", "Скрині, кнопки, важелі, двері з анімацією"),
    ("item", "Предмети", "Анімовані предмети: луки"),
)


def actor_category(entry):
    p = entry.lower()
    if "/heads/" in p:
        return "head"
    if "/humans/" in p:
        return "body"
    if "/monster/" in p:
        return "monster"
    if "/mobsis/" in p:
        return "object"
    if "/items/" in p:
        return "item"
    return "monster"


# Motion clips: the first rule whose words appear in the name wins.
CLIP_CATEGORIES = (
    ("death", "Смерть і поранення", ("_dead", "_down", "death", "_die", "stumble", "_fall", "hurt", "getup", "standup", "knock", "_kill")),
    ("sleep", "Сон і лежання", ("sleep", "_lie", "bed")),
    ("sit", "Сидіння", ("sit", "bench", "throne", "stool")),
    ("dialog", "Діалоги й жести", ("_say_", "talk", "gesture", "dialog", "listen", "_yes", "_no_", "point", "wave", "cheer", "laugh", "threat", "bow_", "salute", "shrug")),
    ("attack", "Бій: атаки", ("attack", "combo", "_hit", "strike", "shoot", "cast", "spell", "throw", "magic")),
    ("defend", "Бій: захист і ухили", ("parade", "block", "jumpback", "jumpleft", "jumpright", "dodge", "evade", "recover")),
    ("move", "Пересування", ("move", "walk", "_run", "sprint", "turn", "jump", "swim", "climb", "sneak", "strafe", "_step")),
    ("interact", "Взаємодія з предметами", ("consume", "drink", "eat", "_play", "forge", "anvil", "alchemy", "chest", "lever", "pick", "dig", "cook", "pipe", "smoke", "button", "door", "read", "pray", "dance", "grind", "saw", "sweep", "fish", "_use")),
    ("idle", "Стійка (idle)", ("ambient", "idle", "_stand_")),
)


def clip_category(name):
    n = name.lower()
    for key, _, words in CLIP_CATEGORIES:
        if any(w in n for w in words):
            return key
    return "other"


def enum_items(categories, with_all=True):
    items = [(ALL, "Усі", "Без фільтра")] if with_all else []
    return items + [(k, label, desc) for k, label, desc in categories]


CLIP_ENUM = enum_items(tuple((k, label, "Кліпи: " + ", ".join(w.strip("_") for w in words[:6])) for k, label, words in CLIP_CATEGORIES)
                       + (("other", "Інше", "Усе, що не впало в інші категорії"),))

_meshes = None
_actors = None


def meshes():
    """(name, entry, category) of every static model and SpeedTree plant."""
    global _meshes
    if _meshes is None:
        _meshes = [(f["name"], f["entry"], mesh_category(f["entry"])) for f in core.run("find", core.game_dir(), "", "1000000")]
        _meshes += [(f["name"], f["entry"], "trees") for f in core.run("trees", core.game_dir(), "")]
    return _meshes


def is_tree(name):
    return any(n == name and c == "trees" for n, _, c in meshes())


def actors():
    """(name, entry, category) of every actor."""
    global _actors
    if _actors is None:
        _actors = [(f["name"], f["entry"], actor_category(f["entry"])) for f in core.run("actors", core.game_dir(), "", "100000")]
    return _actors


def search(rows, category, text, limit=300):
    q = text.lower()
    return [n for n, _, c in rows if (category == ALL or c == category) and q in n.lower()][:limit]


def reset():
    global _meshes, _actors
    _meshes = _actors = None
