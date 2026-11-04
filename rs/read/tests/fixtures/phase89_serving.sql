-- Installed only in fresh disposable databases by the market serving suite.
-- The buyer, seller, administrator and inventory item come from the race seed;
-- the second administrator receives support capabilities, and one legacy
-- commission row gives the administrator a commission to update.
CREATE OR REPLACE FUNCTION serving_seed() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM races_seed();
    PERFORM commission_fixture_rows(1);
    INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password)
    VALUES('00000000-0000-0000-0000-000000003002','serving-support@example.test','unused','ADMIN','APPROVED',true,false);
END $$;
