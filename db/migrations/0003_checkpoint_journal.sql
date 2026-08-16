CREATE TABLE `checkpoint_journal` (
	`session_id` text NOT NULL,
	`turn_count` integer NOT NULL,
	`phase` text NOT NULL,
	`project_path` text NOT NULL,
	`created_at` integer NOT NULL,
	`last_error` text,
	PRIMARY KEY(`session_id`, `turn_count`, `phase`)
);
