curl https://example.com/v1/chat/completions \
  -H "Authorization: Bearer fixture-secret-token" \
  -H "Content-Type: application/json" \
  -d '{"model":"fixture-model","temperature":0.2,"stream":true}'
