# GitHub: быстрая установка без сборки на сервере

## 1. Создай репозиторий

1. https://github.com/new → имя, например `arizona-parser`
2. **Private** (рекомендую) или Public
3. НЕ добавляй README/gitignore (они уже есть)

## 2. Залей код (на Windows, в папке проекта)

```powershell
cd "C:\Users\admin\Desktop\Парсер форума"
git init
git add .
git commit -m "init"
git branch -M main
git remote add origin https://github.com/ТВОЙ_НИК/arizona-parser.git
git push -u origin main
```

`.gitignore` уже исключает секреты: `secret.key`, `users.json`, `cookies.json`, `state.db`, `profiles/`, `target/`.

## 3. Дождись сборки

Вкладка **Actions** → workflow **build-linux** → зелёная галочка (2-3 минуты).

Скачать бинарник:
- **Actions** → последний запуск → **Artifacts** → `arizona-parser-linux`
- или создай тег и получи ссылку на релиз:
```powershell
git tag v1
git push origin v1
```
Тогда файл появится в **Releases** по адресу:
`https://github.com/ТВОЙ_НИК/arizona-parser/releases/download/v1/arizona-parser`

## Важно: config.toml НЕ в репозитории

В git лежит только `config.example.toml` (шаблон). Настоящий `config.toml`
с токеном ВК заливай на сервер **отдельно** через SFTP — как и остальные секреты.

## 4. Установка на сервере (без компиляции!)

```bash
mkdir -p /opt/arizona-parser && cd /opt/arizona-parser
curl -L -o arizona-parser https://github.com/ТВОЙ_НИК/arizona-parser/releases/download/v1/arizona-parser
chmod +x arizona-parser
```

Теперь нужны **секреты** — их в GitHub нет (и правильно). Залей через Termius SFTP в `/opt/arizona-parser`:
- `config.toml` (настоящий, с токеном!)
- `secret.key`
- `users.json`
- `cookies.json`
- `sections.txt`
- `recipients.txt`
- `state.db` (по желанию)

Установи Chrome и создай сервис:

```bash
apt-get update -qq && apt-get install -y -qq wget   libnss3 libatk-bridge2.0-0 libatk1.0-0 libcups2 libdrm2 libxkbcommon0   libxcomposite1 libxdamage1 libxfixes3 libxrandr2 libgbm1 libpango-1.0-0   libcairo2 libasound2t64 fonts-liberation
wget -q -O /tmp/chrome.deb https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
apt-get install -y -qq /tmp/chrome.deb
```

Сервис:

```bash
cat > /etc/systemd/system/arizona-parser.service <<'EOF'
[Unit]
Description=Arizona RP parser bot
After=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/arizona-parser
User=root
Environment=RUST_LOG=info
ExecStart=/opt/arizona-parser/arizona-parser
Restart=always
RestartSec=10
StandardOutput=append:/opt/arizona-parser/bot.log
StandardError=append:/opt/arizona-parser/bot.log

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now arizona-parser
```

## 5. Обновление в будущем

```bash
cd /opt/arizona-parser
systemctl stop arizona-parser
curl -L -o arizona-parser https://github.com/ТВОЙ_НИК/arizona-parser/releases/download/v2/arizona-parser
chmod +x arizona-parser
systemctl start arizona-parser
```

## Важно

- **Никогда не коммить** `secret.key`, `users.json`, `cookies.json` — это доступ к аккаунтам.
- Репозиторий лучше **Private**.
- Первая сборка в Actions ~3-5 мин, дальше из-за кэша ~1-2 мин.
