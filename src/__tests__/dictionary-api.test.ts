import { beforeEach, describe, expect, it, vi } from 'vitest';

const client = {
  getCanonical: vi.fn(),
};

vi.mock('../client.js', () => ({
  getClient: () => client,
}));

import { dictionaryApi } from '../api/dictionary.js';

describe('dictionaryApi', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    client.getCanonical.mockResolvedValue({ data: [] });
  });

  it('lists the account-scoped catalog with type and search query params', async () => {
    await dictionaryApi.list({
      type: 'event',
      q: 'checkout',
      limit: 20,
      offset: 0,
    });

    expect(client.getCanonical).toHaveBeenCalledWith('/dictionary', {
      type: 'event',
      q: 'checkout',
      limit: 20,
      offset: 0,
    });
  });

  it('omits undefined list filters', async () => {
    await dictionaryApi.list({ type: 'prop' });

    expect(client.getCanonical).toHaveBeenCalledWith('/dictionary', {
      type: 'prop',
    });
  });

  it('looks up one subject by subject_id query param', async () => {
    await dictionaryApi.get('event:checkout_completed');
    await dictionaryApi.get('source/table with spaces');

    expect(client.getCanonical).toHaveBeenNthCalledWith(
      1,
      '/dictionary/lookup',
      { subject_id: 'event:checkout_completed' },
    );
    expect(client.getCanonical).toHaveBeenNthCalledWith(
      2,
      '/dictionary/lookup',
      { subject_id: 'source/table with spaces' },
    );
  });
});
