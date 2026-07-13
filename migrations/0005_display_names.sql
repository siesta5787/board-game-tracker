-- Optional first/last name, shown instead of the (unique, login-only)
-- username wherever a person is displayed to others. Nullable — falls back
-- to username until someone sets it.
ALTER TABLE users ADD COLUMN first_name TEXT;
ALTER TABLE users ADD COLUMN last_name TEXT;
