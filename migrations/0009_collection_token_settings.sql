ALTER TABLE collection_tokens
    ADD COLUMN name TEXT;

UPDATE collection_tokens
SET name = 'Collection token ' || left(id::text, 8);

ALTER TABLE collection_tokens
    ALTER COLUMN name SET NOT NULL,
    ADD CONSTRAINT collection_tokens_name_length
        CHECK (char_length(name) BETWEEN 1 AND 100),
    ALTER COLUMN expires_at DROP NOT NULL,
    ALTER COLUMN expires_at DROP DEFAULT;

DROP INDEX collection_tokens_one_active_per_suite_idx;
