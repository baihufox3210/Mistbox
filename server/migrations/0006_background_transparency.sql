-- Start the unified overlay/panel transparency control fully transparent.
-- Keep the stored value as opacity for API compatibility.
UPDATE settings SET value = '0' WHERE key = 'background_dim';
