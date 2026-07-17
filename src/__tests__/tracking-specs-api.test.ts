import { beforeEach, describe, expect, it, vi } from 'vitest';

const client = {
  getCanonical: vi.fn(),
  postCanonical: vi.fn(),
  patchCanonical: vi.fn(),
  deleteCanonical: vi.fn(),
};

vi.mock('../client.js', () => ({
  getClient: () => client,
}));

import { trackingSpecsApi } from '../api/tracking-specs.js';

describe('trackingSpecsApi', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('uses the canonical account-scoped collection and item paths', async () => {
    client.getCanonical.mockResolvedValue({ data: [] });
    client.postCanonical.mockResolvedValue({ data: {} });
    client.patchCanonical.mockResolvedValue({ data: {} });
    client.deleteCanonical.mockResolvedValue({ data: {} });

    await trackingSpecsApi.list({ status: 'active', limit: 20 });
    await trackingSpecsApi.get('spec/with spaces');
    await trackingSpecsApi.create({ name: 'Lifecycle' });
    await trackingSpecsApi.update('spec-1', {
      expectedRevision: 4,
      name: 'Lifecycle v2',
    });
    await trackingSpecsApi.archive('spec-1');

    expect(client.getCanonical).toHaveBeenNthCalledWith(1, '/specs', {
      status: 'active',
      limit: 20,
    });
    expect(client.getCanonical).toHaveBeenNthCalledWith(
      2,
      '/specs/spec%2Fwith%20spaces',
    );
    expect(client.postCanonical).toHaveBeenNthCalledWith(1, '/specs', {
      name: 'Lifecycle',
    });
    expect(client.patchCanonical).toHaveBeenCalledWith('/specs/spec-1', {
      expectedRevision: 4,
      name: 'Lifecycle v2',
    });
    expect(client.deleteCanonical).toHaveBeenCalledWith('/specs/spec-1');
  });

  it('maps publish, versions, diff, and validation results exactly', async () => {
    client.getCanonical.mockResolvedValue({ data: [] });
    client.postCanonical.mockResolvedValue({ data: {} });

    await trackingSpecsApi.publish('spec-1', {
      expectedRevision: 7,
      changelog: 'Ready',
    });
    await trackingSpecsApi.versions('spec-1');
    await trackingSpecsApi.versions('spec-1', true);
    await trackingSpecsApi.diff('spec-1', 'version-2', 'version-5');
    await trackingSpecsApi.validationResults('spec-1', {
      status: 'failed',
    });

    expect(client.postCanonical).toHaveBeenCalledWith('/specs/spec-1/publish', {
      expectedRevision: 7,
      changelog: 'Ready',
    });
    expect(client.getCanonical).toHaveBeenNthCalledWith(
      1,
      '/specs/spec-1/versions',
    );
    expect(client.getCanonical).toHaveBeenNthCalledWith(
      2,
      '/specs/spec-1/versions',
      {
        include_payload: true,
      },
    );
    expect(client.getCanonical).toHaveBeenNthCalledWith(
      3,
      '/specs/spec-1/diff',
      {
        from_version_id: 'version-2',
        to_version_id: 'version-5',
      },
    );
    expect(client.getCanonical).toHaveBeenNthCalledWith(
      4,
      '/specs/spec-1/validation-results',
      { status: 'failed' },
    );
  });
});
