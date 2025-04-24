"use client"

import { useState } from "react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { toast } from "sonner"
import { Loader2 } from "lucide-react"

interface RestoreWalletFormProps {
  isLoading: boolean
  setIsLoading: (loading: boolean) => void
  onWalletRestored: (walletId: string, onchainAddress: string, offchainAddress: string) => void
}

export function RestoreWalletForm({ isLoading, setIsLoading, onWalletRestored }: RestoreWalletFormProps) {
  const [walletId, setWalletId] = useState("")

  const handleRestoreWallet = async () => {
    if (!walletId.trim()) {
      toast.error("Error", {
        description: "Please enter a wallet ID",
      })
      return
    }

    try {
      setIsLoading(true)

      // Get addresses to verify wallet exists
      const addressResponse = await fetch(`http://localhost:8080/get_address/${walletId}`, {
        method: "GET",
      })

      if (!addressResponse.ok) {
        throw new Error("Failed to restore wallet. Invalid wallet ID.")
      }

      const addressData = await addressResponse.json()
      const { onchain_address, offchain_address } = addressData

      if (!onchain_address || !offchain_address) {
        throw new Error("Invalid wallet data received")
      }

      onWalletRestored(walletId, onchain_address, offchain_address)
    } catch (error) {
      console.error("Error restoring wallet:", error)
      toast.error("Error", {
        description: error instanceof Error ? error.message : "Failed to restore wallet",
      })
    } finally {
      setIsLoading(false)
    }
  }

  return (
    <div className="flex flex-col space-y-6">
      <div className="space-y-2">
        <Label htmlFor="wallet-id">Wallet ID</Label>
        <Input
          id="wallet-id"
          placeholder="Enter your wallet ID"
          value={walletId}
          onChange={(e) => setWalletId(e.target.value)}
          className="bg-zinc-900 border-zinc-800"
        />
      </div>

      <Button onClick={handleRestoreWallet} disabled={isLoading} className="bg-orange-600 hover:bg-orange-700">
        {isLoading ? (
          <>
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            Restoring Wallet...
          </>
        ) : (
          "Restore Wallet"
        )}
      </Button>
    </div>
  )
}
