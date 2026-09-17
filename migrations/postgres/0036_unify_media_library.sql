INSERT INTO media_library_assets (
    user_id,
    title,
    kind,
    path,
    metadata_json,
    source,
    is_public,
    created_at,
    updated_at
)
SELECT
    p.user_id,
    LEFT(p.prompt, 200),
    'image',
    '/api/images/' || p.filename,
    '{}',
    'imported',
    1,
    p.created_at,
    p.created_at
FROM plaza_images p
WHERE NOT EXISTS (
    SELECT 1
    FROM media_library_assets a
    WHERE a.user_id = p.user_id
      AND a.path = '/api/images/' || p.filename
);
