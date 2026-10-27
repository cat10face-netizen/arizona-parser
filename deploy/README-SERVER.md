# Перенос на сервер (Ubuntu 26.04)

## 1. Что нужно перенести

Всё содержимое папки проекта, **обязательно** вместе с:
- `config.toml` — настройки
- `users.json` — пользователи (cookies зашифрованы)
- `secret.key` — **ключ расшифровки cookies (без него cookies не прочитать!)**
- `state.db` — база проверенных тем
- `sections.txt`, `recipients.txt`, `cookies.json`

`profiles/` и `target/` переносить НЕ обязательно (`target` пересоберётся, `profiles` — кэш Chrome).

## 2. Быстрый способ

```bash
# на сервере
apt update && apt install -y unzip
mkdir -p /opt/arizona-parser
cd /opt/arizona-parser
# загрузи архив (scp / sftp / git) и распакуй
unzip parser.zip
chmod +x install.sh run.command
./install.sh          # ставит Chromium, Rust и собирает
```

## 3. Настройки под Linux (`config.toml`)

```toml
HEADLESS = true
CHROME_PATH = "/usr/bin/chromium"     # или /usr/bin/google-chrome
```

Проверить, где chrome:
```bash
which chromium chromium-browser google-chrome 2>/dev/null
```

## 4. Запуск вручную

```bash
cd /opt/arizona-parser
./run.command
```

Или в фоне:
```bash
nohup ./arizona-parser > bot.log 2>&1 &
```

## 5. Автозапуск (systemd)

```bash
cp deploy/arizona-parser.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now arizona-parser
systemctl status arizona-parser
```

Логи:
```bash
journalctl -u arizona-parser -f
# или
tail -f /opt/arizona-parser/bot.err.log
```

## 6. Важно

- **`secret.key` потеряешь — cookies не расшифруются.** Сделай бэкап.
- Не запускай бота одновременно на Windows и на сервере с одним `state.db` — будет дубль уведомлений.
- На сервере Chrome работает в headless, окна нет — это нормально.
- Если Chrome падает с `Browser process exited`, проверь, что установлены зависимости:
  ```bash
  apt install -y libnss3 libatk-bridge2.0-0 libdrm2 libxkbcommon0 libgbm1 libasound2t64
  ```
