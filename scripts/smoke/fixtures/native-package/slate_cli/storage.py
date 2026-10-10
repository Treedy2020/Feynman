class InMemoryTokenStore:
    def __init__(self, values=None): self.values = values or {}
    def get(self, profile): return self.values.get(profile)
    def set(self, profile, value): self.values[profile] = value

class KeyringTokenStore:
    def get(self, profile): return 'FAKE_ACCESS_TOKEN_MUST_NOT_LEAK'
