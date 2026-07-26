from openai import OpenAI

client = OpenAI(
    base_url="https://integrate.api.nvidia.com/v1",
    api_key="nvapi-fixture-secret",
)
client.chat.completions.create(
    model="z-ai/glm-5.2",
    temperature=1,
    top_p=1,
    max_tokens=16384,
    seed=42,
    stream=True,
)
