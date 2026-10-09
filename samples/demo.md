---
title: Hello, Marathon
---

Press `r` to fetch sample JSON, choose a name, and run the next command.

```sh
curl -fsS --max-time 10 https://jsonplaceholder.typicode.com/todos/1
printf '\n'
```

```json mrthn=input
{
  "type": "select",
  "prompt": "Who are you?",
  "target": "NAME",
  "options": ["Alice", "Bob"]
}
```

```sh
echo "Hello, $NAME!"
```
