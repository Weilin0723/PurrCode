import OpenAI from "openai";
const client = new OpenAI({
  baseURL: "http://127.0.0.1:1234/v1",
  apiKey: process.env.LM_STUDIO_API_KEY,
});
await client.chat.completions.create({ model: "local-model", stream: true });
